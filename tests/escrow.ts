import * as anchor from "@coral-xyz/anchor";
import { Keypair, PublicKey, SystemProgram, SYSVAR_RENT_PUBKEY } from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  getAssociatedTokenAddress,
} from "@solana/spl-token";
import { expect } from "chai";
import {
  airdrop,
  BN,
  claimPda,
  configPda,
  createFakeUsdcMint,
  ensureAtaWithBalance,
  expectError,
  makeFundedKeypair,
  randomId16,
  taskPda,
  tokenBalance,
  usdc,
} from "./utils";

describe("clustly-escrow", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program: anchor.Program<any> = anchor.workspace.ClustlyEscrow;

  // Shared state across describe blocks
  let admin: Keypair;
  let mintAuthority: Keypair;
  let poster: Keypair;
  let claimer1: Keypair;
  let claimer2: Keypair;
  let claimer3: Keypair;
  let usdcMint: PublicKey;
  let posterAta: PublicKey;
  let configPdaKey: PublicKey;
  let feeVaultAta: PublicKey;

  const DELIVERABLE_HASH = Buffer.alloc(32, 7); // arbitrary

  before(async () => {
    admin = await makeFundedKeypair(provider, 20);
    mintAuthority = await makeFundedKeypair(provider, 5);
    poster = await makeFundedKeypair(provider, 20);
    claimer1 = await makeFundedKeypair(provider, 5);
    claimer2 = await makeFundedKeypair(provider, 5);
    claimer3 = await makeFundedKeypair(provider, 5);

    usdcMint = await createFakeUsdcMint(provider, mintAuthority);

    // Mint 10_000 USDC to poster for test tasks.
    posterAta = await ensureAtaWithBalance(
      provider,
      usdcMint,
      poster.publicKey,
      poster,
      mintAuthority,
      usdc(10_000)
    );

    [configPdaKey] = configPda(program.programId);
    feeVaultAta = await getAssociatedTokenAddress(usdcMint, configPdaKey, true);

    // Initialize config once.
    await program.methods
      .initializeConfig(admin.publicKey, 400)
      .accountsStrict({
        payer: admin.publicKey,
        config: configPdaKey,
        usdcMint,
        feeVault: feeVaultAta,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .signers([admin])
      .rpc();

    const cfg = await program.account.escrowConfig.fetch(configPdaKey);
    expect(cfg.feeBps).to.equal(400);
    expect(cfg.admin.toBase58()).to.equal(admin.publicKey.toBase58());
    expect(cfg.usdcMint.toBase58()).to.equal(usdcMint.toBase58());
  });

  /* ────────────────────────────────────────────────────── */

  async function initTask(opts: {
    taskId?: Buffer;
    bountyUsdc?: number;
    maxSlots?: number;
    claimDeadlineSecs?: number;
    taskDeadlineTs?: number; // unix seconds; 0 = no deadline
  }): Promise<{ taskId: Buffer; taskPdaKey: PublicKey; taskVault: PublicKey }> {
    const taskId = opts.taskId ?? randomId16();
    const bounty = opts.bountyUsdc ?? 5;
    const maxSlots = opts.maxSlots ?? 1;
    const claimDeadlineSecs = opts.claimDeadlineSecs ?? 48 * 3600;
    const taskDeadlineTs = opts.taskDeadlineTs ?? 0;

    const [taskPdaKey] = taskPda(program.programId, taskId);
    const taskVault = await getAssociatedTokenAddress(usdcMint, taskPdaKey, true);

    await program.methods
      .initializeTask(
        Array.from(taskId),
        new BN(usdc(bounty).toString()),
        maxSlots,
        claimDeadlineSecs,
        new BN(taskDeadlineTs)
      )
      .accountsStrict({
        poster: poster.publicKey,
        config: configPdaKey,
        usdcMint,
        task: taskPdaKey,
        taskVault,
        posterAta,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .signers([poster])
      .rpc();

    return { taskId, taskPdaKey, taskVault };
  }

  async function enrollClaim(
    taskPdaKey: PublicKey,
    claimer: Keypair
  ): Promise<{ claimId: Buffer; claimPdaKey: PublicKey }> {
    const claimId = randomId16();
    const [claimPdaKey] = claimPda(program.programId, claimId);

    await program.methods
      .enroll(Array.from(claimId))
      .accountsStrict({
        claimer: claimer.publicKey,
        task: taskPdaKey,
        claim: claimPdaKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([claimer])
      .rpc();

    return { claimId, claimPdaKey };
  }

  async function submitDeliverable(
    taskPdaKey: PublicKey,
    claimPdaKey: PublicKey,
    claimer: Keypair
  ): Promise<void> {
    await program.methods
      .submit(Array.from(DELIVERABLE_HASH))
      .accountsStrict({
        claimer: claimer.publicKey,
        task: taskPdaKey,
        claim: claimPdaKey,
      })
      .signers([claimer])
      .rpc();
  }

  async function approve(
    taskPdaKey: PublicKey,
    taskVault: PublicKey,
    claimPdaKey: PublicKey,
    claimer: Keypair
  ): Promise<{ claimerAta: PublicKey }> {
    const claimerAta = await getAssociatedTokenAddress(usdcMint, claimer.publicKey);
    await program.methods
      .approveClaim()
      .accountsStrict({
        poster: poster.publicKey,
        config: configPdaKey,
        usdcMint,
        task: taskPdaKey,
        claim: claimPdaKey,
        taskVault,
        claimerAta,
        claimer: claimer.publicKey,
        feeVault: feeVaultAta,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .signers([poster])
      .rpc();
    return { claimerAta };
  }

  async function reject(
    taskPdaKey: PublicKey,
    taskVault: PublicKey,
    claimPdaKey: PublicKey
  ): Promise<void> {
    await program.methods
      .rejectClaim()
      .accountsStrict({
        poster: poster.publicKey,
        config: configPdaKey,
        task: taskPdaKey,
        claim: claimPdaKey,
        taskVault,
        posterAta,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([poster])
      .rpc();
  }

  /* ────────────────────────────────────────────────────── */

  describe("happy path: single-slot approve with fee split", () => {
    it("init → enroll → submit → approve → fee auto-routed", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 5 });

      // Vault holds 5 USDC after deposit.
      expect(await tokenBalance(provider, taskVault)).to.equal(usdc(5));

      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);

      const feeVaultBefore = await tokenBalance(provider, feeVaultAta);
      const { claimerAta } = await approve(taskPdaKey, taskVault, claimPdaKey, claimer1);

      const claimerBal = await tokenBalance(provider, claimerAta);
      const vaultAfter = await tokenBalance(provider, taskVault);
      const feeVaultAfter = await tokenBalance(provider, feeVaultAta);

      expect(claimerBal).to.equal(usdc(4.8), "claimer gets 96%");
      expect(vaultAfter).to.equal(0n, "vault drained");
      expect(feeVaultAfter - feeVaultBefore).to.equal(usdc(0.2), "fee_vault got 4%");

      const claim = await program.account.escrowClaim.fetch(claimPdaKey);
      expect(claim.status).to.equal(2, "status = Approved(2)");
    });
  });

  describe("multi-slot with partial fill and close_task", () => {
    it("3 slots, 2 approved, close_task refunds the 3rd", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 3, maxSlots: 3 });
      expect(await tokenBalance(provider, taskVault)).to.equal(usdc(9));

      const c1 = await enrollClaim(taskPdaKey, claimer1);
      const c2 = await enrollClaim(taskPdaKey, claimer2);

      await submitDeliverable(taskPdaKey, c1.claimPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, c2.claimPdaKey, claimer2);

      await approve(taskPdaKey, taskVault, c1.claimPdaKey, claimer1);
      await approve(taskPdaKey, taskVault, c2.claimPdaKey, claimer2);

      const posterBefore = await tokenBalance(provider, posterAta);

      // close_task (poster signs, 3rd slot never filled)
      await program.methods
        .closeTask()
        .accountsStrict({
          caller: poster.publicKey,
          config: configPdaKey,
          task: taskPdaKey,
          taskVault,
          posterAta,
          poster: poster.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([poster])
        .rpc();

      const posterAfter = await tokenBalance(provider, posterAta);
      expect(posterAfter - posterBefore).to.equal(usdc(3), "1 unfilled slot refunded");

      const task = await program.account.escrowTask.fetch(taskPdaKey);
      expect(task.status).to.equal(1, "task closed");
      expect(task.slotsApproved).to.equal(2);
    });
  });

  describe("resubmit flow (submit accepts Rejected)", () => {
    it("enroll → submit → reject → submit again → approve", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 2 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer2);

      await submitDeliverable(taskPdaKey, claimPdaKey, claimer2);
      await reject(taskPdaKey, taskVault, claimPdaKey);

      let claim = await program.account.escrowClaim.fetch(claimPdaKey);
      expect(claim.status).to.equal(3, "status = Rejected");
      expect(claim.rejections).to.equal(1);

      // Resubmit — the same `submit` instruction, from Rejected state
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer2);
      claim = await program.account.escrowClaim.fetch(claimPdaKey);
      expect(claim.status).to.equal(1, "back to Submitted");

      const { claimerAta } = await approve(taskPdaKey, taskVault, claimPdaKey, claimer2);
      const bal = await tokenBalance(provider, claimerAta);
      expect(bal >= usdc(1.92)).to.equal(true, `expected >= 1.92 USDC, got ${bal}`);
    });
  });

  describe("3 rejections → claim cancelled + slot refunded", () => {
    it("refunds bounty to poster on 3rd rejection", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 4 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer3);

      const posterBefore = await tokenBalance(provider, posterAta);

      for (let i = 0; i < 3; i++) {
        await submitDeliverable(taskPdaKey, claimPdaKey, claimer3);
        await reject(taskPdaKey, taskVault, claimPdaKey);
      }

      const claim = await program.account.escrowClaim.fetch(claimPdaKey);
      expect(claim.rejections).to.equal(3);
      expect(claim.status).to.equal(4, "status = Cancelled");

      const posterAfter = await tokenBalance(provider, posterAta);
      expect(posterAfter - posterBefore).to.equal(usdc(4), "bounty refunded after 3 rejects");
    });
  });

  describe("cancel before enroll", () => {
    it("refunds full bounty to poster", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 7 });
      const posterBefore = await tokenBalance(provider, posterAta);

      await program.methods
        .cancelBeforeEnroll()
        .accountsStrict({
          poster: poster.publicKey,
          config: configPdaKey,
          task: taskPdaKey,
          taskVault,
          posterAta,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([poster])
        .rpc();

      const posterAfter = await tokenBalance(provider, posterAta);
      expect(posterAfter - posterBefore).to.equal(usdc(7));
    });

    it("fails if any slot enrolled", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 5 });
      await enrollClaim(taskPdaKey, claimer1);

      await expectError(
        () =>
          program.methods
            .cancelBeforeEnroll()
            .accountsStrict({
              poster: poster.publicKey,
              config: configPdaKey,
              task: taskPdaKey,
              taskVault,
              posterAta,
              tokenProgram: TOKEN_PROGRAM_ID,
            })
            .signers([poster])
            .rpc(),
        "TaskHasEnrollments"
      );
    });
  });

  describe("permissionless close_task after deadline", () => {
    it("anyone can close after task_deadline with no enrollments (auto_cancel path)", async () => {
      const past = Math.floor(Date.now() / 1000) - 10;
      const { taskPdaKey, taskVault } = await initTask({
        bountyUsdc: 3,
        taskDeadlineTs: past,
      });

      const stranger = await makeFundedKeypair(provider, 1);
      const posterBefore = await tokenBalance(provider, posterAta);

      await program.methods
        .autoCancelTask()
        .accountsStrict({
          caller: stranger.publicKey,
          config: configPdaKey,
          task: taskPdaKey,
          taskVault,
          posterAta,
          poster: poster.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([stranger])
        .rpc();

      const posterAfter = await tokenBalance(provider, posterAta);
      expect(posterAfter - posterBefore).to.equal(usdc(3));
    });

    it("close_task by stranger fails before deadline", async () => {
      const future = Math.floor(Date.now() / 1000) + 3600;
      const { taskPdaKey, taskVault } = await initTask({
        bountyUsdc: 1,
        taskDeadlineTs: future,
      });

      const stranger = await makeFundedKeypair(provider, 1);
      await expectError(
        () =>
          program.methods
            .closeTask()
            .accountsStrict({
              caller: stranger.publicKey,
              config: configPdaKey,
              task: taskPdaKey,
              taskVault,
              posterAta,
              poster: poster.publicKey,
              tokenProgram: TOKEN_PROGRAM_ID,
            })
            .signers([stranger])
            .rpc(),
        "DeadlineNotReached"
      );
    });
  });

  describe("dispute: admin-resolved for deliverer", () => {
    it("reject → open_dispute → resolve_dispute(Deliverer) → payout", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 10 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);
      await reject(taskPdaKey, taskVault, claimPdaKey);

      await program.methods
        .openDispute()
        .accountsStrict({
          claimer: claimer1.publicKey,
          task: taskPdaKey,
          claim: claimPdaKey,
        })
        .signers([claimer1])
        .rpc();

      let claim = await program.account.escrowClaim.fetch(claimPdaKey);
      expect(claim.status).to.equal(5, "Disputed");

      const claimerAta = await getAssociatedTokenAddress(usdcMint, claimer1.publicKey);
      const claimerBefore = await tokenBalance(provider, claimerAta);

      await program.methods
        .resolveDispute({ deliverer: {} })
        .accountsStrict({
          admin: admin.publicKey,
          config: configPdaKey,
          usdcMint,
          task: taskPdaKey,
          claim: claimPdaKey,
          taskVault,
          claimer: claimer1.publicKey,
          claimerAta,
          posterAta,
          feeVault: feeVaultAta,
          systemProgram: SystemProgram.programId,
          tokenProgram: TOKEN_PROGRAM_ID,
          associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
          rent: SYSVAR_RENT_PUBKEY,
        })
        .signers([admin])
        .rpc();

      const claimerAfter = await tokenBalance(provider, claimerAta);
      expect(claimerAfter - claimerBefore).to.equal(usdc(9.6), "deliverer +96%");

      claim = await program.account.escrowClaim.fetch(claimPdaKey);
      expect(claim.status).to.equal(6, "ResolvedDeliverer");
    });
  });

  describe("dispute: admin-resolved for poster", () => {
    it("resolve_dispute(Poster) refunds the slot", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 6 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer2);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer2);
      await reject(taskPdaKey, taskVault, claimPdaKey);

      await program.methods
        .openDispute()
        .accountsStrict({
          claimer: claimer2.publicKey,
          task: taskPdaKey,
          claim: claimPdaKey,
        })
        .signers([claimer2])
        .rpc();

      const claimerAta = await getAssociatedTokenAddress(usdcMint, claimer2.publicKey);
      const posterBefore = await tokenBalance(provider, posterAta);

      await program.methods
        .resolveDispute({ poster: {} })
        .accountsStrict({
          admin: admin.publicKey,
          config: configPdaKey,
          usdcMint,
          task: taskPdaKey,
          claim: claimPdaKey,
          taskVault,
          claimer: claimer2.publicKey,
          claimerAta,
          posterAta,
          feeVault: feeVaultAta,
          systemProgram: SystemProgram.programId,
          tokenProgram: TOKEN_PROGRAM_ID,
          associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
          rent: SYSVAR_RENT_PUBKEY,
        })
        .signers([admin])
        .rpc();

      const posterAfter = await tokenBalance(provider, posterAta);
      expect(posterAfter - posterBefore).to.equal(usdc(6), "poster +full bounty");

      const claim = await program.account.escrowClaim.fetch(claimPdaKey);
      expect(claim.status).to.equal(7, "ResolvedPoster");
    });
  });

  describe("admin rotation", () => {
    it("update_admin: old admin rejected, new admin works", async () => {
      const newAdmin = await makeFundedKeypair(provider, 5);

      await program.methods
        .updateAdmin(newAdmin.publicKey)
        .accountsStrict({
          config: configPdaKey,
          admin: admin.publicKey,
        })
        .signers([admin])
        .rpc();

      // Old admin now rejected
      await expectError(
        () =>
          program.methods
            .updateFee(450)
            .accountsStrict({
              config: configPdaKey,
              admin: admin.publicKey,
            })
            .signers([admin])
            .rpc(),
        "Unauthorized"
      );

      // New admin can update fee
      await program.methods
        .updateFee(450)
        .accountsStrict({
          config: configPdaKey,
          admin: newAdmin.publicKey,
        })
        .signers([newAdmin])
        .rpc();

      // Restore state for subsequent tests
      await program.methods
        .updateFee(400)
        .accountsStrict({
          config: configPdaKey,
          admin: newAdmin.publicKey,
        })
        .signers([newAdmin])
        .rpc();
      await program.methods
        .updateAdmin(admin.publicKey)
        .accountsStrict({
          config: configPdaKey,
          admin: newAdmin.publicKey,
        })
        .signers([newAdmin])
        .rpc();
    });
  });

  describe("fee withdrawal (admin can only touch fee_vault)", () => {
    it("withdraw_fees transfers from fee_vault to destination", async () => {
      const feeBal = await tokenBalance(provider, feeVaultAta);
      expect(feeBal > 0n).to.equal(
        true,
        `fee_vault should have accumulated fees, got ${feeBal}`
      );

      const destAta = await ensureAtaWithBalance(
        provider,
        usdcMint,
        admin.publicKey,
        admin,
        mintAuthority,
        0n
      );

      const destBefore = await tokenBalance(provider, destAta);
      const withdrawAmount = usdc(0.1);

      await program.methods
        .withdrawFees(new BN(withdrawAmount.toString()))
        .accountsStrict({
          admin: admin.publicKey,
          config: configPdaKey,
          feeVault: feeVaultAta,
          destination: destAta,
          usdcMint,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([admin])
        .rpc();

      const destAfter = await tokenBalance(provider, destAta);
      expect(destAfter - destBefore).to.equal(withdrawAmount);
    });

    it("withdraw_fees fails when amount > fee_vault balance", async () => {
      const destAta = await ensureAtaWithBalance(
        provider,
        usdcMint,
        admin.publicKey,
        admin,
        mintAuthority,
        0n
      );
      await expectError(
        () =>
          program.methods
            .withdrawFees(new BN(usdc(100_000).toString()))
            .accountsStrict({
              admin: admin.publicKey,
              config: configPdaKey,
              feeVault: feeVaultAta,
              destination: destAta,
              usdcMint,
              tokenProgram: TOKEN_PROGRAM_ID,
            })
            .signers([admin])
            .rpc(),
        "InsufficientFeeVault"
      );
    });
  });

  describe("negative paths", () => {
    it("self-enrollment blocked", async () => {
      const { taskPdaKey } = await initTask({ bountyUsdc: 1 });
      const claimId = randomId16();
      const [claimPdaKey] = claimPda(program.programId, claimId);

      await expectError(
        () =>
          program.methods
            .enroll(Array.from(claimId))
            .accountsStrict({
              claimer: poster.publicKey,
              task: taskPdaKey,
              claim: claimPdaKey,
              systemProgram: SystemProgram.programId,
            })
            .signers([poster])
            .rpc(),
        "SelfEnrollment"
      );
    });

    it("non-poster cannot approve", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);

      const claimerAta = await getAssociatedTokenAddress(usdcMint, claimer1.publicKey);
      await expectError(
        () =>
          program.methods
            .approveClaim()
            .accountsStrict({
              poster: claimer1.publicKey, // wrong signer
              config: configPdaKey,
              usdcMint,
              task: taskPdaKey,
              claim: claimPdaKey,
              taskVault,
              claimerAta,
              claimer: claimer1.publicKey,
              feeVault: feeVaultAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([claimer1])
            .rpc(),
        "Unauthorized"
      );
    });

    it("fee bps > 500 rejected", async () => {
      await expectError(
        () =>
          program.methods
            .updateFee(600)
            .accountsStrict({
              config: configPdaKey,
              admin: admin.publicKey,
            })
            .signers([admin])
            .rpc(),
        "FeeTooHigh"
      );
    });

    it("submit from non-existent state (Approved) fails", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);
      await approve(taskPdaKey, taskVault, claimPdaKey, claimer1);

      await expectError(
        () => submitDeliverable(taskPdaKey, claimPdaKey, claimer1),
        "InvalidStateTransition"
      );
    });

    it("initialize_task with zero bounty rejected", async () => {
      const taskId = randomId16();
      const [taskPdaKey] = taskPda(program.programId, taskId);
      const taskVault = await getAssociatedTokenAddress(usdcMint, taskPdaKey, true);

      await expectError(
        () =>
          program.methods
            .initializeTask(Array.from(taskId), new BN(0), 1, 48 * 3600, new BN(0))
            .accountsStrict({
              poster: poster.publicKey,
              config: configPdaKey,
              usdcMint,
              task: taskPdaKey,
              taskVault,
              posterAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([poster])
            .rpc(),
        "ZeroBounty"
      );
    });
  });

  describe("deliverer ATA init on approve (critical gap from eng review)", () => {
    it("approve succeeds for claimer with no prior USDC ATA", async () => {
      const freshClaimer = await makeFundedKeypair(provider, 2);
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, freshClaimer);
      await submitDeliverable(taskPdaKey, claimPdaKey, freshClaimer);

      const claimerAta = await getAssociatedTokenAddress(usdcMint, freshClaimer.publicKey);
      // Precondition: ATA doesn't exist
      expect(await tokenBalance(provider, claimerAta)).to.equal(0n);

      await program.methods
        .approveClaim()
        .accountsStrict({
          poster: poster.publicKey,
          config: configPdaKey,
          usdcMint,
          task: taskPdaKey,
          claim: claimPdaKey,
          taskVault,
          claimerAta,
          claimer: freshClaimer.publicKey,
          feeVault: feeVaultAta,
          systemProgram: SystemProgram.programId,
          tokenProgram: TOKEN_PROGRAM_ID,
          associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
          rent: SYSVAR_RENT_PUBKEY,
        })
        .signers([poster])
        .rpc();

      // ATA created and funded
      expect(await tokenBalance(provider, claimerAta)).to.equal(usdc(0.96));
    });
  });

  /* ══════════════════════════════════════════════════════════════
     ADVERSARIAL SUITE — every exploit scenario.
     Each test sets up a legitimate state then tries an attack.
     Every test must demonstrate the program rejects cleanly.
     ══════════════════════════════════════════════════════════════ */

  describe("adversarial: authority bypass", () => {
    it("non-admin cannot withdraw_fees", async () => {
      const attacker = await makeFundedKeypair(provider, 2);
      const attackerAta = await ensureAtaWithBalance(
        provider, usdcMint, attacker.publicKey, attacker, mintAuthority, 0n
      );
      await expectError(
        () =>
          program.methods
            .withdrawFees(new BN(usdc(0.1).toString()))
            .accountsStrict({
              admin: attacker.publicKey,
              config: configPdaKey,
              feeVault: feeVaultAta,
              destination: attackerAta,
              usdcMint,
              tokenProgram: TOKEN_PROGRAM_ID,
            })
            .signers([attacker])
            .rpc(),
        "Unauthorized"
      );
    });

    it("non-admin cannot resolve_dispute", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 2 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);
      await reject(taskPdaKey, taskVault, claimPdaKey);
      await program.methods
        .openDispute()
        .accountsStrict({ claimer: claimer1.publicKey, task: taskPdaKey, claim: claimPdaKey })
        .signers([claimer1])
        .rpc();

      const attacker = await makeFundedKeypair(provider, 2);
      const claimerAta = await getAssociatedTokenAddress(usdcMint, claimer1.publicKey);
      await expectError(
        () =>
          program.methods
            .resolveDispute({ deliverer: {} })
            .accountsStrict({
              admin: attacker.publicKey,
              config: configPdaKey,
              usdcMint,
              task: taskPdaKey,
              claim: claimPdaKey,
              taskVault,
              claimer: claimer1.publicKey,
              claimerAta,
              posterAta,
              feeVault: feeVaultAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([attacker])
            .rpc(),
        "Unauthorized"
      );
    });

    it("non-admin cannot update_admin", async () => {
      const attacker = await makeFundedKeypair(provider, 2);
      await expectError(
        () =>
          program.methods
            .updateAdmin(attacker.publicKey)
            .accountsStrict({ config: configPdaKey, admin: attacker.publicKey })
            .signers([attacker])
            .rpc(),
        "Unauthorized"
      );
    });

    it("non-poster cannot reject_claim", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);
      const attacker = await makeFundedKeypair(provider, 2);
      const attackerAta = await ensureAtaWithBalance(
        provider, usdcMint, attacker.publicKey, attacker, mintAuthority, 0n
      );
      await expectError(
        () =>
          program.methods
            .rejectClaim()
            .accountsStrict({
              poster: attacker.publicKey,
              config: configPdaKey,
              task: taskPdaKey,
              claim: claimPdaKey,
              taskVault,
              posterAta: attackerAta,
              tokenProgram: TOKEN_PROGRAM_ID,
            })
            .signers([attacker])
            .rpc(),
        "Unauthorized"
      );
    });

    it("non-poster cannot cancel_before_enroll", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const attacker = await makeFundedKeypair(provider, 2);
      const attackerAta = await ensureAtaWithBalance(
        provider, usdcMint, attacker.publicKey, attacker, mintAuthority, 0n
      );
      await expectError(
        () =>
          program.methods
            .cancelBeforeEnroll()
            .accountsStrict({
              poster: attacker.publicKey,
              config: configPdaKey,
              task: taskPdaKey,
              taskVault,
              posterAta: attackerAta,
              tokenProgram: TOKEN_PROGRAM_ID,
            })
            .signers([attacker])
            .rpc(),
        "Unauthorized"
      );
    });

    it("non-claimer cannot submit another claimer's claim", async () => {
      const { taskPdaKey } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await expectError(
        () =>
          program.methods
            .submit(Array.from(DELIVERABLE_HASH))
            .accountsStrict({
              claimer: claimer2.publicKey,
              task: taskPdaKey,
              claim: claimPdaKey,
            })
            .signers([claimer2])
            .rpc(),
        "Unauthorized"
      );
    });

    it("non-claimer cannot open_dispute", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);
      await reject(taskPdaKey, taskVault, claimPdaKey);
      await expectError(
        () =>
          program.methods
            .openDispute()
            .accountsStrict({ claimer: claimer2.publicKey, task: taskPdaKey, claim: claimPdaKey })
            .signers([claimer2])
            .rpc(),
        "Unauthorized"
      );
    });
  });

  describe("adversarial: account substitution", () => {
    it("cannot pass attacker's token account as fee_vault in approve_claim", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);

      const attacker = await makeFundedKeypair(provider, 2);
      const attackerAta = await ensureAtaWithBalance(
        provider, usdcMint, attacker.publicKey, attacker, mintAuthority, 0n
      );
      const claimerAta = await getAssociatedTokenAddress(usdcMint, claimer1.publicKey);

      await expectError(
        () =>
          program.methods
            .approveClaim()
            .accountsStrict({
              poster: poster.publicKey,
              config: configPdaKey,
              usdcMint,
              task: taskPdaKey,
              claim: claimPdaKey,
              taskVault,
              claimerAta,
              claimer: claimer1.publicKey,
              feeVault: attackerAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([poster])
            .rpc(),
        "Constraint"
      );
    });

    it("CRITICAL: cannot pass task_vault as fee_vault in withdraw_fees", async () => {
      const { taskVault } = await initTask({ bountyUsdc: 5 });
      const destAta = await ensureAtaWithBalance(
        provider, usdcMint, admin.publicKey, admin, mintAuthority, 0n
      );
      await expectError(
        () =>
          program.methods
            .withdrawFees(new BN(usdc(1).toString()))
            .accountsStrict({
              admin: admin.publicKey,
              config: configPdaKey,
              feeVault: taskVault,
              destination: destAta,
              usdcMint,
              tokenProgram: TOKEN_PROGRAM_ID,
            })
            .signers([admin])
            .rpc(),
        "Constraint"
      );
    });

    it("cannot redirect payout via non-canonical claimer_ata", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);

      const attacker = await makeFundedKeypair(provider, 2);
      const attackerAta = await ensureAtaWithBalance(
        provider, usdcMint, attacker.publicKey, attacker, mintAuthority, 0n
      );

      await expectError(
        () =>
          program.methods
            .approveClaim()
            .accountsStrict({
              poster: poster.publicKey,
              config: configPdaKey,
              usdcMint,
              task: taskPdaKey,
              claim: claimPdaKey,
              taskVault,
              claimerAta: attackerAta,
              claimer: claimer1.publicKey,
              feeVault: feeVaultAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([poster])
            .rpc(),
        "Constraint"
      );
    });

    it("cannot pass wrong mint in approve_claim", async () => {
      const otherMint = await createFakeUsdcMint(provider, mintAuthority);
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);
      const claimerAta = await getAssociatedTokenAddress(usdcMint, claimer1.publicKey);
      await expectError(
        () =>
          program.methods
            .approveClaim()
            .accountsStrict({
              poster: poster.publicKey,
              config: configPdaKey,
              usdcMint: otherMint,
              task: taskPdaKey,
              claim: claimPdaKey,
              taskVault,
              claimerAta,
              claimer: claimer1.publicKey,
              feeVault: feeVaultAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([poster])
            .rpc(),
        "Constraint"
      );
    });
  });

  describe("adversarial: cross-task confusion", () => {
    it("cannot approve task_A's claim using task_B's PDA", async () => {
      const taskA = await initTask({ bountyUsdc: 5 });
      const taskB = await initTask({ bountyUsdc: 5 });
      const { claimPdaKey } = await enrollClaim(taskA.taskPdaKey, claimer1);
      await submitDeliverable(taskA.taskPdaKey, claimPdaKey, claimer1);

      const claimerAta = await getAssociatedTokenAddress(usdcMint, claimer1.publicKey);
      await expectError(
        () =>
          program.methods
            .approveClaim()
            .accountsStrict({
              poster: poster.publicKey,
              config: configPdaKey,
              usdcMint,
              task: taskB.taskPdaKey,
              claim: claimPdaKey,
              taskVault: taskB.taskVault,
              claimerAta,
              claimer: claimer1.publicKey,
              feeVault: feeVaultAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([poster])
            .rpc(),
        "TaskClaimMismatch"
      );
    });

    it("cannot submit with wrong task", async () => {
      const taskA = await initTask({ bountyUsdc: 1 });
      const taskB = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskA.taskPdaKey, claimer1);
      await expectError(
        () =>
          program.methods
            .submit(Array.from(DELIVERABLE_HASH))
            .accountsStrict({
              claimer: claimer1.publicKey,
              task: taskB.taskPdaKey,
              claim: claimPdaKey,
            })
            .signers([claimer1])
            .rpc(),
        "TaskClaimMismatch"
      );
    });
  });

  describe("adversarial: state machine bypass", () => {
    it("cannot approve from Enrolled (must submit first)", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      const claimerAta = await getAssociatedTokenAddress(usdcMint, claimer1.publicKey);
      await expectError(
        () =>
          program.methods
            .approveClaim()
            .accountsStrict({
              poster: poster.publicKey,
              config: configPdaKey,
              usdcMint,
              task: taskPdaKey,
              claim: claimPdaKey,
              taskVault,
              claimerAta,
              claimer: claimer1.publicKey,
              feeVault: feeVaultAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([poster])
            .rpc(),
        "InvalidStateTransition"
      );
    });

    it("cannot double-approve the same claim", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 2 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);
      await approve(taskPdaKey, taskVault, claimPdaKey, claimer1);
      const claimerAta = await getAssociatedTokenAddress(usdcMint, claimer1.publicKey);
      await expectError(
        () =>
          program.methods
            .approveClaim()
            .accountsStrict({
              poster: poster.publicKey,
              config: configPdaKey,
              usdcMint,
              task: taskPdaKey,
              claim: claimPdaKey,
              taskVault,
              claimerAta,
              claimer: claimer1.publicKey,
              feeVault: feeVaultAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([poster])
            .rpc(),
        "InvalidStateTransition"
      );
    });

    it("cannot reject an approved claim", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);
      await approve(taskPdaKey, taskVault, claimPdaKey, claimer1);
      await expectError(
        () => reject(taskPdaKey, taskVault, claimPdaKey),
        "InvalidStateTransition"
      );
    });

    it("cannot open_dispute from Enrolled", async () => {
      const { taskPdaKey } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await expectError(
        () =>
          program.methods
            .openDispute()
            .accountsStrict({ claimer: claimer1.publicKey, task: taskPdaKey, claim: claimPdaKey })
            .signers([claimer1])
            .rpc(),
        "InvalidStateTransition"
      );
    });

    it("cannot open_dispute twice", async () => {
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);
      await reject(taskPdaKey, taskVault, claimPdaKey);
      await program.methods
        .openDispute()
        .accountsStrict({ claimer: claimer1.publicKey, task: taskPdaKey, claim: claimPdaKey })
        .signers([claimer1])
        .rpc();
      await expectError(
        () =>
          program.methods
            .openDispute()
            .accountsStrict({ claimer: claimer1.publicKey, task: taskPdaKey, claim: claimPdaKey })
            .signers([claimer1])
            .rpc(),
        "InvalidStateTransition"
      );
    });

    it("cannot enroll after max_slots reached", async () => {
      const { taskPdaKey } = await initTask({ bountyUsdc: 1, maxSlots: 1 });
      await enrollClaim(taskPdaKey, claimer1);
      await expectError(() => enrollClaim(taskPdaKey, claimer2), "SlotsFull");
    });
  });

  describe("adversarial: re-entry / re-init", () => {
    it("cannot re-initialize config PDA", async () => {
      const attacker = await makeFundedKeypair(provider, 3);
      await expectError(
        () =>
          program.methods
            .initializeConfig(attacker.publicKey, 0)
            .accountsStrict({
              payer: attacker.publicKey,
              config: configPdaKey,
              usdcMint,
              feeVault: feeVaultAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([attacker])
            .rpc(),
        "already in use"
      );
    });

    it("cannot re-initialize an existing task_id", async () => {
      const taskId = randomId16();
      await initTask({ taskId, bountyUsdc: 1 });
      await expectError(() => initTask({ taskId, bountyUsdc: 1 }), "already in use");
    });
  });

  describe("adversarial: math & fee cap", () => {
    it("fee_bps = 0 works (no fee transfer)", async () => {
      await program.methods
        .updateFee(0)
        .accountsStrict({ config: configPdaKey, admin: admin.publicKey })
        .signers([admin])
        .rpc();

      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 1 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);

      const feeBefore = await tokenBalance(provider, feeVaultAta);
      const { claimerAta } = await approve(taskPdaKey, taskVault, claimPdaKey, claimer1);
      const feeAfter = await tokenBalance(provider, feeVaultAta);
      const claimerBal = await tokenBalance(provider, claimerAta);

      expect(feeAfter - feeBefore).to.equal(0n);
      expect(claimerBal >= usdc(1)).to.equal(true);

      await program.methods
        .updateFee(400)
        .accountsStrict({ config: configPdaKey, admin: admin.publicKey })
        .signers([admin])
        .rpc();
    });

    it("fee_bps at cap (500) works", async () => {
      await program.methods
        .updateFee(500)
        .accountsStrict({ config: configPdaKey, admin: admin.publicKey })
        .signers([admin])
        .rpc();

      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 2 });
      const { claimPdaKey } = await enrollClaim(taskPdaKey, claimer1);
      await submitDeliverable(taskPdaKey, claimPdaKey, claimer1);

      const feeBefore = await tokenBalance(provider, feeVaultAta);
      await approve(taskPdaKey, taskVault, claimPdaKey, claimer1);
      const feeAfter = await tokenBalance(provider, feeVaultAta);

      expect(feeAfter - feeBefore).to.equal(usdc(0.1));

      await program.methods
        .updateFee(400)
        .accountsStrict({ config: configPdaKey, admin: admin.publicKey })
        .signers([admin])
        .rpc();
    });

    it("fee_bps = 501 rejected (cap enforcement)", async () => {
      await expectError(
        () =>
          program.methods
            .updateFee(501)
            .accountsStrict({ config: configPdaKey, admin: admin.publicKey })
            .signers([admin])
            .rpc(),
        "FeeTooHigh"
      );
    });

    it("bounty * max_slots overflow rejected", async () => {
      const taskId = randomId16();
      const [taskPdaKey] = taskPda(program.programId, taskId);
      const taskVault = await getAssociatedTokenAddress(usdcMint, taskPdaKey, true);
      const HUGE = new BN("18446744073709551615");
      await expectError(
        () =>
          program.methods
            .initializeTask(Array.from(taskId), HUGE, 2, 48 * 3600, new BN(0))
            .accountsStrict({
              poster: poster.publicKey,
              config: configPdaKey,
              usdcMint,
              task: taskPdaKey,
              taskVault,
              posterAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([poster])
            .rpc(),
        "MathOverflow"
      );
    });

    it("max_slots = 0 rejected", async () => {
      const taskId = randomId16();
      const [taskPdaKey] = taskPda(program.programId, taskId);
      const taskVault = await getAssociatedTokenAddress(usdcMint, taskPdaKey, true);
      await expectError(
        () =>
          program.methods
            .initializeTask(Array.from(taskId), new BN(usdc(1).toString()), 0, 48 * 3600, new BN(0))
            .accountsStrict({
              poster: poster.publicKey,
              config: configPdaKey,
              usdcMint,
              task: taskPdaKey,
              taskVault,
              posterAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([poster])
            .rpc(),
        "InvalidSlots"
      );
    });

    it("max_slots = 101 rejected", async () => {
      const taskId = randomId16();
      const [taskPdaKey] = taskPda(program.programId, taskId);
      const taskVault = await getAssociatedTokenAddress(usdcMint, taskPdaKey, true);
      await expectError(
        () =>
          program.methods
            .initializeTask(Array.from(taskId), new BN(usdc(0.01).toString()), 101, 48 * 3600, new BN(0))
            .accountsStrict({
              poster: poster.publicKey,
              config: configPdaKey,
              usdcMint,
              task: taskPdaKey,
              taskVault,
              posterAta,
              systemProgram: SystemProgram.programId,
              tokenProgram: TOKEN_PROGRAM_ID,
              associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
              rent: SYSVAR_RENT_PUBKEY,
            })
            .signers([poster])
            .rpc(),
        "InvalidSlots"
      );
    });
  });

  describe("adversarial: donation doesn't break anything", () => {
    it("extra USDC donated to task_vault refunds to poster on cancel", async () => {
      const { mintTo } = await import("@solana/spl-token");
      const { taskPdaKey, taskVault } = await initTask({ bountyUsdc: 2 });

      await mintTo(
        provider.connection,
        poster,
        usdcMint,
        taskVault,
        mintAuthority,
        Number(usdc(100))
      );
      expect(await tokenBalance(provider, taskVault)).to.equal(usdc(102));

      const posterBefore = await tokenBalance(provider, posterAta);
      await program.methods
        .cancelBeforeEnroll()
        .accountsStrict({
          poster: poster.publicKey,
          config: configPdaKey,
          task: taskPdaKey,
          taskVault,
          posterAta,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([poster])
        .rpc();
      const posterAfter = await tokenBalance(provider, posterAta);
      expect(posterAfter - posterBefore).to.equal(usdc(102));
    });
  });
});
