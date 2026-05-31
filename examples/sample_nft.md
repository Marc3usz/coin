# Sample NFT Contract

Deploy `examples/sample_nft.litevm` from the TUI Deploy tab.

ABI mappings to add in the ABI Wizard after adding the deployed contract address to the Address Book:

| Method ID | Name | Args | Rets |
| --- | --- | --- | --- |
| 0 | init | 0 | 0 |
| 1 | mint | 2 | 0 |
| 2 | transfer | 2 | 0 |
| 3 | ownerOf | 1 | 1 |

Manual flow:

1. Deploy `examples/sample_nft.litevm`.
2. Mine the deploy transaction.
3. Add the expected contract address in Address Book.
4. Add the ABI rows above in ABI Wizard.
5. Call `init` once before minting.
6. Call `mint` with args: `1, 0xYOUR_ADDRESS`.
7. Mine the call transaction.
8. Call `ownerOf` with args: `1`.
9. Call `transfer` with args: `1, 0xRECIPIENT_ADDRESS` from the current owner wallet.

The contract stores token ownership in a persistent map at state field `0` and emits text logs: `init`, `mint`, and `transfer`.

Regenerate the hex with:

```powershell
cargo run --example sample_nft
```
