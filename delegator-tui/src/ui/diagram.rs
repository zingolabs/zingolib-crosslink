//! A flow diagram: labelled lines entering a box from the left, leaving it on the right.
//!
//! Pure text in, pure text out, so the shape can be tested without a terminal.

use crate::format::truncate_end;

/// One diagram. Both lists render top down; `box_lines` is centred inside the box.
pub struct Flow<'a> {
    pub left: &'a [String],
    pub right: &'a [String],
    pub box_lines: &'a [String],
    /// Shown in place of the left list when it is empty.
    pub left_empty: &'a str,
    /// Shown in place of the right list when it is empty.
    pub right_empty: &'a str,
}

/// Narrowest a side may be squeezed to when space runs out. Below this the diagram is
/// skipped rather than drawn with unreadable labels.
const MIN_SIDE: usize = 14;
/// Rows one side draws before collapsing the remainder into a count.
const MAX_ROWS: usize = 8;
/// Columns each side spends on the gap, the lead, the bus, and the stub.
const CONNECTOR: usize = 5;

/// What a piece of the diagram is, so the caller can colour it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Spacing.
    Pad,
    /// Lines between the labels and the box.
    Wire,
    /// The box outline.
    Frame,
    /// A line of text inside the box, by index into `box_lines`.
    Title(usize),
    /// A left label, by index into `left`.
    Left(usize),
    /// A right label, by index into `right`.
    Right(usize),
    /// The placeholder for an empty side, or the count of rows folded away.
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub part: Part,
    pub text: String,
}

/// Collects one row's pieces, merging neighbours of the same part.
#[derive(Default)]
struct Row(Vec<Piece>);

impl Row {
    fn push(&mut self, part: Part, text: impl Into<String>) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        let part = if text.trim().is_empty() {
            Part::Pad
        } else {
            part
        };
        match self.0.last_mut() {
            Some(last) if last.part == part => last.text.push_str(&text),
            _ => self.0.push(Piece { part, text }),
        }
    }

    /// Drops trailing spacing so rows never carry invisible padding.
    fn finish(mut self) -> Vec<Piece> {
        while self.0.last().is_some_and(|p| p.part == Part::Pad) {
            self.0.pop();
        }
        if let Some(last) = self.0.last_mut() {
            last.text.truncate(last.text.trim_end().len());
        }
        self.0
    }
}

impl Flow<'_> {
    /// Renders the diagram as rows of tagged pieces, or nothing when `width` is too small.
    pub fn render_parts(&self, width: usize) -> Vec<Vec<Piece>> {
        let (left, left_folded) = cap(self.left);
        let (right, right_folded) = cap(self.right);
        let box_w = self
            .box_lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0)
            .max(11)
            + 2;
        // each side is as wide as its longest label, so the diagram starts at the left edge
        // and is only as wide as it needs to be
        let natural = |labels: &[String], empty: &str| {
            let widest = labels.iter().map(|l| l.chars().count()).max();
            widest.unwrap_or_else(|| empty.chars().count())
        };
        let (want_left, want_right) = (
            natural(&left, self.left_empty),
            natural(&right, self.right_empty),
        );
        let Some((left_w, right_w)) = width
            .checked_sub(box_w + 2 + 2 * CONNECTOR)
            .and_then(|room| fit(want_left, want_right, room))
        else {
            return Vec::new();
        };

        let rows = left.len().max(right.len()).max(self.box_lines.len()).max(1);
        let height = rows + 2;
        let middle = 1 + (rows - 1) / 2;
        let entry = 1 + left.len().saturating_sub(1) / 2;
        let exit = 1 + right.len().saturating_sub(1) / 2;
        let box_top = 1 + (rows - self.box_lines.len()) / 2;
        let label_part = |index: usize, len: usize, folded: bool, part: fn(usize) -> Part| {
            if folded && index == len - 1 {
                Part::Note
            } else {
                part(index)
            }
        };

        (0..height)
            .map(|r| {
                let mut row = Row::default();
                let on_left = r >= 1 && r <= left.len();
                let on_right = r >= 1 && r <= right.len();

                let (label, part) = if on_left {
                    (
                        truncate_end(&left[r - 1], left_w),
                        label_part(r - 1, left.len(), left_folded, Part::Left),
                    )
                } else if left.is_empty() && r == middle {
                    (truncate_end(self.left_empty, left_w), Part::Note)
                } else {
                    (String::new(), Part::Pad)
                };
                row.push(Part::Pad, " ".repeat(left_w - label.chars().count()));
                row.push(part, label);
                row.push(Part::Pad, " ");
                if on_left {
                    row.push(Part::Wire, '─'.to_string());
                    row.push(
                        Part::Wire,
                        bus(r, left.len(), entry, Side::Left).to_string(),
                    );
                } else {
                    row.push(Part::Pad, "  ");
                }
                row.push(
                    Part::Wire,
                    if !left.is_empty() && r == entry {
                        "──"
                    } else {
                        "  "
                    },
                );

                if r == 0 || r == height - 1 {
                    let (open, close) = if r == 0 {
                        ('┌', '┐')
                    } else {
                        ('└', '┘')
                    };
                    row.push(Part::Frame, format!("{open}{}{close}", "─".repeat(box_w)));
                } else {
                    row.push(
                        Part::Frame,
                        if !left.is_empty() && r == entry {
                            '┤'
                        } else {
                            '│'
                        }
                        .to_string(),
                    );
                    match r.checked_sub(box_top).filter(|i| *i < self.box_lines.len()) {
                        Some(i) => {
                            let text = truncate_end(&self.box_lines[i], box_w);
                            let pad = box_w - text.chars().count();
                            row.push(Part::Pad, " ".repeat(pad / 2));
                            row.push(Part::Title(i), text);
                            row.push(Part::Pad, " ".repeat(pad - pad / 2));
                        }
                        None => row.push(Part::Pad, " ".repeat(box_w)),
                    }
                    row.push(
                        Part::Frame,
                        if !right.is_empty() && r == exit {
                            '├'
                        } else {
                            '│'
                        }
                        .to_string(),
                    );
                }

                row.push(
                    Part::Wire,
                    if !right.is_empty() && r == exit {
                        "──"
                    } else {
                        "  "
                    },
                );
                if on_right {
                    row.push(
                        Part::Wire,
                        bus(r, right.len(), exit, Side::Right).to_string(),
                    );
                    row.push(Part::Wire, '─'.to_string());
                    row.push(Part::Pad, " ");
                    row.push(
                        label_part(r - 1, right.len(), right_folded, Part::Right),
                        truncate_end(&right[r - 1], right_w),
                    );
                } else if right.is_empty() && r == middle {
                    row.push(Part::Pad, "   ");
                    row.push(Part::Note, truncate_end(self.right_empty, right_w));
                }
                row.finish()
            })
            .collect()
    }

    /// The diagram as plain text, for tests.
    #[cfg(test)]
    pub fn render(&self, width: usize) -> Vec<String> {
        self.render_parts(width)
            .into_iter()
            .map(|row| row.into_iter().map(|p| p.text).collect())
            .collect()
    }
}

/// Widths for the two sides within `room` columns. Each side gets what it wants when both
/// fit; otherwise the side that needs less keeps it and the other takes the rest. `None` when
/// a side would drop below [`MIN_SIDE`] and still be cut.
fn fit(want_left: usize, want_right: usize, room: usize) -> Option<(usize, usize)> {
    let (left, right) = if want_left + want_right <= room {
        (want_left, want_right)
    } else if want_left <= room / 2 {
        (want_left, room - want_left)
    } else if want_right <= room / 2 {
        (room - want_right, want_right)
    } else {
        (room / 2, room - room / 2)
    };
    let readable = |got: usize, want: usize| got >= want || got >= MIN_SIDE;
    (readable(left, want_left) && readable(right, want_right)).then_some((left, right))
}

enum Side {
    Left,
    Right,
}

/// The junction character where one row meets the vertical bus. `join` is the row at which the
/// bus turns towards the box.
fn bus(row: usize, count: usize, join: usize, side: Side) -> char {
    let first = row == 1;
    let last = row == count;
    if first && last {
        return '─';
    }
    match (row == join, first, last, side) {
        (true, true, _, _) => '┬',
        (true, _, true, _) => '┴',
        (true, ..) => '┼',
        (false, true, _, Side::Left) => '┐',
        (false, true, _, Side::Right) => '┌',
        (false, _, true, Side::Left) => '┘',
        (false, _, true, Side::Right) => '└',
        (false, _, _, Side::Left) => '┤',
        (false, _, _, Side::Right) => '├',
    }
}

/// Keeps the first rows and folds the rest into a count, so one huge transaction cannot
/// produce a diagram hundreds of rows tall. Returns whether the last row is that count.
fn cap(items: &[String]) -> (Vec<String>, bool) {
    if items.len() <= MAX_ROWS {
        return (items.to_vec(), false);
    }
    let mut out = items[..MAX_ROWS - 1].to_vec();
    out.push(format!("and {} more", items.len() - (MAX_ROWS - 1)));
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn render(left: &[&str], right: &[&str], width: usize) -> Vec<String> {
        let box_lines = strings(&["transaction", "fee 0.0001"]);
        Flow {
            left: &strings(left),
            right: &strings(right),
            box_lines: &box_lines,
            left_empty: "no wallet inputs",
            right_empty: "no outputs",
        }
        .render(width)
    }

    #[test]
    fn three_in_two_out() {
        let lines = render(&["a 1", "b 2", "c 3"], &["d 4", "e 5"], 80);
        let art = lines.join("\n");
        println!("{art}");
        assert_eq!(lines.len(), 5, "three rows plus two borders");
        // the bus collects three inputs and turns into the box on the middle row
        assert!(lines[1].contains("a 1 ─┐"));
        assert!(lines[2].contains("b 2 ─┼──┤"));
        assert!(lines[3].contains("c 3 ─┘"));
        // the box joins the outgoing bus, which forks to two labels
        assert!(lines[1].contains("├──┬─ d 4"));
        assert!(lines[2].contains("└─ e 5"));
        assert!(art.contains("transaction"));
        assert!(art.contains("fee 0.0001"));
    }

    #[test]
    fn a_single_item_needs_no_bus() {
        let lines = render(&["only 1"], &["out 1"], 80);
        assert!(lines[1].contains("only 1 ────┤"), "{}", lines[1]);
        assert!(lines[1].contains("├──── out 1"), "{}", lines[1]);
    }

    #[test]
    fn an_empty_side_says_so_and_grows_no_bus() {
        let lines = render(&[], &["out 1", "out 2"], 80);
        let art = lines.join("\n");
        assert!(art.contains("no wallet inputs"));
        assert!(!art.contains('┤'), "no entry junction on an empty side");
        assert!(art.contains('├'), "the outgoing side still connects");
    }

    #[test]
    fn both_sides_empty_still_draws_the_box() {
        let lines = render(&[], &[], 80);
        let art = lines.join("\n");
        assert!(art.contains("no wallet inputs") && art.contains("no outputs"));
        assert!(art.contains("transaction"));
    }

    #[test]
    fn long_lists_are_capped() {
        let many: Vec<&str> = (0..20).map(|_| "x 1").collect();
        let lines = render(&many, &[], 80);
        assert_eq!(lines.len(), MAX_ROWS + 2);
        assert!(lines.join("\n").contains("and 13 more"));
    }

    #[test]
    fn a_narrow_terminal_gets_no_diagram() {
        let long = "orchard      0.00020000 ZEC";
        assert!(render(&[long], &[long], 40).is_empty());
        // short labels still fit the same width
        assert!(!render(&["a 1"], &["b 2"], 40).is_empty());
    }

    #[test]
    fn the_diagram_starts_at_the_left_edge_and_is_only_as_wide_as_its_labels() {
        let lines = render(
            &["transparent 0.0002", "orchard 0.0005"],
            &["0.0001 sapling"],
            200,
        );
        let indent = |l: &String| l.chars().take_while(|c| *c == ' ').count();
        assert_eq!(lines.iter().map(indent).min(), Some(0), "anchored left");
        let widest = lines.iter().map(|l| l.chars().count()).max().unwrap();
        assert!(widest < 70, "not stretched across 200 columns: {widest}");
    }

    #[test]
    fn a_squeezed_side_gives_way_before_a_short_one() {
        assert_eq!(fit(10, 10, 40), Some((10, 10)));
        assert_eq!(
            fit(8, 60, 40),
            Some((8, 32)),
            "the short side keeps its width"
        );
        assert_eq!(fit(60, 8, 40), Some((32, 8)));
        assert_eq!(fit(60, 60, 40), Some((20, 20)));
        assert_eq!(fit(60, 60, 20), None, "ten columns each is too few");
    }

    #[test]
    fn pieces_are_tagged_for_colouring() {
        let box_lines = strings(&["sent", "fee 1"]);
        let rows = Flow {
            left: &strings(&["in 1", "in 2"]),
            right: &strings(&["out 1"]),
            box_lines: &box_lines,
            left_empty: "",
            right_empty: "",
        }
        .render_parts(80);
        let parts: Vec<Part> = rows.iter().flatten().map(|p| p.part).collect();
        for part in [
            Part::Left(0),
            Part::Left(1),
            Part::Right(0),
            Part::Title(0),
            Part::Title(1),
            Part::Frame,
            Part::Wire,
        ] {
            assert!(parts.contains(&part), "{part:?} missing");
        }
        let label = rows
            .iter()
            .flatten()
            .find(|p| p.part == Part::Left(1))
            .unwrap();
        assert_eq!(label.text, "in 2");
        assert!(
            rows.iter()
                .all(|row| row.last().is_none_or(|p| p.part != Part::Pad)),
            "no trailing padding"
        );
    }

    #[test]
    fn folded_rows_are_notes_not_labels() {
        let many: Vec<String> = (0..20).map(|i| format!("x {i}")).collect();
        let rows = Flow {
            left: &many,
            right: &[],
            box_lines: &strings(&["tx"]),
            left_empty: "",
            right_empty: "none",
        }
        .render_parts(80);
        let note = rows
            .iter()
            .flatten()
            .find(|p| p.text.contains("more"))
            .unwrap();
        assert_eq!(note.part, Part::Note);
        assert!(!rows.iter().flatten().any(|p| p.part == Part::Left(7)));
    }

    #[test]
    fn rows_never_exceed_the_width() {
        for width in [60, 80, 100, 140] {
            for line in render(&["a 1", "b 2"], &["c 3"], width) {
                assert!(line.chars().count() <= width, "width {width}: {line}");
            }
        }
    }
}
