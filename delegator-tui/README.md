# delegator-tui

A terminal wallet for Zcash crosslink delegators. It opens a wallet from a seed phrase or
watches one through a unified full viewing key, syncs continuously, and shows balances, bonded
funds, transactions with memos, and receiving addresses with QR codes. Sending and staking from
the terminal are not in this version; a seed wallet here is the same wallet as in Zingo
Delegator, which does both.

```text
cargo run --release -p delegator-tui
```

It knows two chains, the crosslink featurenet (`stable`, the live chain) and the crosslink
`nightly`, and nothing else: wallet files for mainnet, the public Zcash testnet, or regtest are
never listed. `--network` picks the chain for new wallets and their indexers: the stable
chain's are the three nodes Shielded Labs hands out for it, `45.63.75.9`, `70.34.220.94` and
`64.177.44.176` on port 18234, the nightly chain's come from zingolib's built-in list, and
`--server` overrides either. `--fake` runs on
a made-up wallet with no network at all, to look at every screen.

## The first screen

Launch shows the crosslink wallets in the data directory: name, chain, kind (seed or watch),
birthday, and when the file was last written, with the sigil of the chosen one. `Enter` opens
one. `n`, or the last row, starts the setup menu: create a new wallet, restore from a seed
phrase, or watch with a viewing key. A seed phrase or a viewing key pasted on the list or the
menu goes straight to the matching branch. With no wallets yet the menu comes first.

- **Create** shows the 24 words, the birthday, and the seed's sigil. The birthday is a hundred
  blocks under the tip, unless the indexer serves no Ironwood tree state there: then it is the
  pool's activation height, block 1, so the wallet can rebuild the tree from scanned blocks.
  The same rule settles a birthday typed on restore or watch, and one given on the command
  line. `Enter`
  writes the wallet; there is no check that the words were written down. `r` shows the words
  on one line, `y` copies the birthday (never the words).
- **Restore** takes the words typed or pasted; a number after them is read as the birthday,
  the way the app puts both on the clipboard.
- **Watch** takes the key from Zingo Delegator (settings › viewing key). The two crosslink
  chains share their keys, so the chain is the one chosen on the menu (`n` switches it).
- **Birthday** can be pasted from the app, typed as a height, given as a date (placed two weeks
  early for safety, using the block spacing measured from the chain), or left unknown, which
  scans from block 1.
- **Name** becomes the file name, `<name>.dat`. Each wallet also gets `<name>.meta` (what the
  list shows) and, when an indexer was given, `<name>.server`.

The key legend is always on the last row, with the chain tip (`T:`) and the chain on its
right. Errors show on the row above it.

`--wallet NAME` opens a wallet without the list. `--ufvk` and `--birthday` create a watch-only
wallet without the screens, named `watch` unless `--wallet` says otherwise.
`--seed-derivation crosslink` derives spending keys the way the zebra-crosslink node does; the
default is standard ZIP-32. See `--help` for the rest.

## Which chain a wallet file is on

Both crosslink chains share their parameters with the public Zcash testnet, so a wallet file
alone says only `test`. The list reads the first bytes of each `.dat` to drop mainnet and
regtest files, then takes the chain from the `.meta` file beside the wallet, else from the
`.server` file when that names a built-in crosslink indexer (a public-testnet indexer drops the
file too). A testnet file with neither is listed as unverified; when it opens, its newest synced
block is asked for from the stable and the nightly indexers and the hash decides the chain. A
file whose block is on neither is refused as a wallet from another network. Wallets with no
blocks yet keep the chain chosen on the menu.

## Screens and keys

Home, Transactions, Receive, Addresses, and Sync. Each opens with the first letter of its name,
from anywhere: `h` `t` `r` `a` `s`. `j` `k` move, `Enter` opens, `Esc` goes back, `c` picks a
colour theme, `?` opens the key list, and `q` quits after saving.

Home shows the spendable balance across the Orchard, Sapling, and transparent pools, and the
amount locked in delegation bonds beneath it. Bond transactions appear in the list as
`bond created`, `unbond begun`, `bond withdrawn`, and `bond retargeted`. A transaction's detail
view switches between its inputs and outputs and its wallet events with `v`.

## Look

The first-run screens are laid out like zingolib-tui's wizard, a title row with the step
count on the right, a rule under it, the body two columns in, and dressed after athanor's
lockscreen: prompts that speak (`@ paste the viewing key:`), input boxes in a double gold
frame, a sigil for each key (an 8×8 glyph from a hash of it,
mirrored so it reads as a figure), and a roguelike status line. With 110 columns and 30 rows
or more, lwtui's dither field fills the left of the screen like a lake; on smaller terminals it
runs in the free rows under the content. `--dither braille` draws it in dots, `--dither off`
removes it. Once a wallet is open, the chrome follows
[lwtui](https://github.com/auzum197/lightwallet-tools/tree/main/tools/tui): a one-line header
with the shimmering title, a status mark and the screen names; a pane with a rule on its left;
dim key hints on the last line; the field in the free part of the home screen.

The default theme is [srcery](https://srcery.sh/).

The big digits of the balance on a tall home screen are
[Jacquard 24](https://fonts.google.com/specimen/Jacquard+24), the pixel blackletter by the
Soft Type Project, rasterised at its native size into half-block cells: eight rows tall, with
its old-style figures hanging below and rising above the line. The face is under the SIL Open
Font License 1.1; the bitmaps in `ui/jacquard.rs` derive from it. On terminals under thirty
rows the balance keeps the small three-row glyphs.

## Faker mode

`--fake` replaces the wallet task with one that makes everything up: three wallets on the
list, a history with every transaction kind (payments with memos, bonds created, unbonded,
withdrawn and retargeted), bonded funds, addresses of both kinds, and a sync that scans,
finds transactions as you watch, finishes, rests and starts again. New addresses, pause, resume
and rescan all answer. The data comes from a fixed seed, so a screen looks the same on every
run. Nothing is written to disk.

## Themes

Srcery, Lightwallet, Nord, Catppuccin (Latte, Frappé, Macchiato, Mocha), Vitesse (Dark, Light,
Black), Vercel (Dark, Light), Gruvbox (Dark, Light), your terminal's own colours, and monochrome. Press
`c` to open the picker: moving previews the theme live, `Enter` keeps and saves it, `Esc`
reverts.

The choice is saved in `delegator-tui/config` under the platform configuration directory. The
starting theme is the first of `--theme`, the saved choice, monochrome when `NO_COLOR` is set,
and Srcery.

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
