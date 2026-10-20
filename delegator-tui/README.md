# delegator-tui

A terminal wallet for Zcash crosslink delegators. It opens a wallet from a seed phrase or
watches one through a unified full viewing key, syncs continuously, and shows balances, bonded
funds, transactions with memos, and receiving addresses with QR codes. Sending and staking from
the terminal are not in this version; a seed wallet here is the same wallet as in Zingo
Delegator, which does both.

```text
cargo run --release -p delegator-tui
```

## The first screen

Launch shows the wallets in the data directory: name, network, kind (seed or watch), birthday,
and when the file was last written. `Enter` opens one. `n`, or the last row, starts the setup
menu: create a new wallet, restore from a seed phrase, or watch with a viewing key. A seed
phrase or a viewing key pasted on the list or the menu goes straight to the matching branch.
With no wallets yet the menu comes first.

- **Create** shows the 24 words and the birthday together. `Enter` writes the wallet; there is
  no check that the words were written down. `r` shows them on one line, `y` copies the
  birthday (never the words). `n` on the menu picks the network for a seed wallet.
- **Restore** takes the words typed or pasted; a number after them is read as the birthday,
  the way the app puts both on the clipboard.
- **Watch** takes the key from Zingo Delegator (settings › viewing key). Its prefix names the
  network.
- **Birthday** can be pasted from the app, typed as a height, given as a date (placed two weeks
  early for safety, from the chain tip the server reports), or left unknown, which scans from
  the start of Sapling.
- **Name** becomes the file name, `<name>.dat`. Each wallet also gets `<name>.meta` (what the
  list shows) and, when a server was given, `<name>.server`.

`--wallet NAME` opens a wallet without the list. `--ufvk` and `--birthday` create a watch-only
wallet without the screens, named `watch` unless `--wallet` says otherwise. The testnet here is
the crosslink testnet, so its server must be a crosslink node; the default zec.rocks testnet
server does not serve that chain. `--seed-derivation crosslink` derives spending keys the way
the zebra-crosslink node does; the default is standard ZIP-32. See `--help` for the rest.

## Screens and keys

Home, Transactions, Receive, Addresses, and Sync. Each opens with the first letter of its name,
from anywhere: `h` `t` `r` `a` `s`. `j` `k` move, `Enter` opens, `Esc` goes back, `c` picks a
colour theme, `?` opens the key list, and `q` quits after saving.

Home shows the spendable balance across the Orchard, Sapling, and transparent pools, and the
amount locked in delegation bonds beneath it. Bond transactions appear in the list as
`bond created`, `unbond begun`, `bond withdrawn`, and `bond retargeted`. A transaction's detail
view switches between its inputs and outputs and its value transfers with `v`.

## Look

The first-run screens are laid out like zingolib-tui's wizard: a title row with the step count
on the right, a rule under it, the body two columns in, `> ` fields with a block cursor, the
key hints under the field, and errors in bold beneath. Once a wallet is open, the chrome
follows [lwtui](https://github.com/auzum197/lightwallet-tools/tree/main/tools/tui), the
lightwallet-tools block explorer: a one-line header with the shimmering title, a status mark
and the screen names; a pane with a rule on its left; dim key hints on the last line. Its warm
palette is the default theme, `lightwallet`.

`--experimental-dither` paints lwtui's drifting dither field over the empty part of the home
screen, in shade blocks; `--experimental-dither-braille` uses braille dots, finer where the
terminal font has them. Both need a 24-bit colour terminal and a palette theme.

## Themes

Lightwallet, Nord, Catppuccin (Latte, Frappé, Macchiato, Mocha), Vitesse (Dark, Light, Black),
Vercel (Dark, Light), Gruvbox (Dark, Light), your terminal's own colours, and monochrome. Press
`c` to open the picker: moving previews the theme live, `Enter` keeps and saves it, `Esc`
reverts.

The choice is saved in `delegator-tui/config` under the platform configuration directory. The
starting theme is the first of `--theme`, the saved choice, monochrome when `NO_COLOR` is set,
and Lightwallet.

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
