# delegator-tui

A watch-only terminal wallet for Zcash crosslink delegators. It imports a unified full viewing
key, syncs continuously, and shows balances, bonded funds, transactions with memos, and
receiving addresses with QR codes. It never holds a spending key and cannot spend or stake.

```text
cargo run --release -p delegator-tui
```

The first launch asks for the viewing key, whose prefix names the network, then the server and
the birthday height. A birthday far below the key's real creation height makes the first sync
slow. The server is saved beside the wallet file, and later launches open the saved wallet
with it. The testnet here is the crosslink testnet, so the server must be a crosslink node;
the default zec.rocks testnet server does not serve that chain. Pass `--ufvk`
and `--birthday` to skip the first-run screen. See `--help` for the rest.

The screens are Home, Transactions, Receive, Addresses, and Sync. Each opens with the first
letter of its name, from anywhere: `h` `t` `r` `a` `s`. `j` `k` move, `Enter` opens, `Esc`
goes back, `c` picks a colour theme, `?` lists everything, and `q` quits after saving.

Home shows the spendable balance across the Orchard, Sapling, and transparent pools, and the
amount locked in delegation bonds beneath it. Bond transactions appear in the list as
`bond created`, `unbond begun`, `bond withdrawn`, and `bond retargeted`. A transaction's detail
view switches between its inputs and outputs and its value transfers with `v`.

## Themes

Nord, Catppuccin (Latte, Frappé, Macchiato, Mocha), Vitesse (Dark, Light, Black), Vercel
(Dark, Light), Gruvbox (Dark, Light), your terminal's own colours, and monochrome. Press `c`
to open the picker: moving previews the theme live, `Enter` keeps and saves it, `Esc` reverts.

The choice is saved in `delegator-tui/config` under the platform configuration directory. The
starting theme is the first of `--theme`, the saved choice, monochrome when `NO_COLOR` is set,
and Gruvbox Dark.

Colours are 24-bit when `COLORTERM` says the terminal supports it, and mapped to the nearest
of the 256 xterm colours otherwise. Force either with `--color-depth 24bit` or
`--color-depth 256`. QR codes use each theme's own dark and light tones, pushed apart until
they reach at least 12:1 contrast, so they match the theme and still scan.

Logs go to `delegator-tui.log` in the data directory. `RUST_LOG` sets the level.

## Sync progress

pepper-sync on this branch has no event stream, so the wallet task reads the sync status twice
a second and reports what changed: the scan plan, the counts of scanned blocks and outputs,
new transactions, and the chain tip. The output total behind the progress bar is derived from
the percentage the engine reports, so it is an estimate until the session finishes.

## Tests

```text
cargo test -p delegator-tui
```

The unit tests need no wallet and no terminal.
