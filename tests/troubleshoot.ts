import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { RenderNetwork } from "../target/types/render_network";
import { PublicKey, Keypair, SystemProgram } from "@solana/Account";
import { createMint, createAccount, mintTo } from "@solana/spl-token";
import { getAssociatedTokenAddressSync, ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_PROGRAM_ID } from "@solana/spl-token";

describe("troubleshoot_withdraw", () => {
  anchor.setProvider(anchor.AnchorProvider.env());
  const provider = anchor.getProvider() as anchor.AnchorProvider;
  const program = anchor.workspace.RenderNetwork as Program<RenderNetwork>;

  const configKeypair = Keypair.generate();
  const feeCollector = Keypair.generate();
  const user = Keypair.generate();
  let mint: PublicKey;
  let userTokenAccount: PublicKey;

  it("Is initialized!", async () => {
    const airdropSig = await provider.connection.requestAirdrop(user.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
    await provider.connection.confirmTransaction(airdropSig);

    mint = await createMint(provider.connection, user, user.publicKey, null, 6);
    userTokenAccount = await createAccount(provider.connection, user, mint, user.publicKey);
    await mintTo(provider.connection, user, mint, userTokenAccount, user, 1000 * 1e6);

    const [userAccountPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("user_account_v2"), user.publicKey.toBuffer()],
      program.programId
    );
    const pdaDepositAta = getAssociatedTokenAddressSync(mint, userAccountPda, true);

    console.log("Depositing...");
    await program.methods
      .depositToAccount(user.publicKey, new anchor.BN(100 * 1e6))
      .accounts({
        userAccount: userAccountPda,
        user: user.publicKey,
        mint: mint,
        userTokenAccount: userTokenAccount,
        userDepositTokenAccount: pdaDepositAta,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([user])
      .rpc();

    console.log("Withdrawing...");
    try {
        await program.methods
            .withdrawFromAccount(new anchor.BN(50 * 1e6))
            .accounts({
                user: user.publicKey,
                userAccount: userAccountPda,
                mint: mint,
                userTokenAccount: userTokenAccount,
                userDepositTokenAccount: pdaDepositAta,
                tokenProgram: TOKEN_PROGRAM_ID,
                associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
                systemProgram: anchor.web3.SystemProgram.programId,
            })
            .signers([user])
            .rpc();
        console.log("Withdrawal OK");
    } catch(err: any) {
        console.log("Withdrawal Failed:", err.logs || err);
        throw err;
    }
  });
});
