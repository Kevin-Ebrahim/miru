use ratatui::layout::Rect;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct TextSelection {
    pub anchor: TextPosition,
    pub end: TextPosition,
    pub dragging: bool,
}

impl TextSelection {
    pub fn range(&self) -> (TextPosition, TextPosition) {
        (self.anchor.min(self.end), self.anchor.max(self.end))
    }
}

pub struct TextView {
    pub area: Rect,
    pub offset: usize,
    pub lines: Vec<String>,
}

impl TextView {
    pub fn position(&self, column: u16, row: u16) -> TextPosition {
        use unicode_width::UnicodeWidthStr;

        let line =
            self.offset + usize::from(row.clamp(self.area.y, self.area.bottom() - 1) - self.area.y);
        let requested =
            usize::from(column.saturating_sub(self.area.x)).min(usize::from(self.area.width));
        let mut position = 0;

        for grapheme in self.lines[line].graphemes(true) {
            if position + grapheme.width() > requested {
                break;
            }
            position += grapheme.width();
        }

        TextPosition {
            line,
            column: position,
        }
    }

    pub fn selected_text(&self, selection: TextSelection) -> String {
        use unicode_width::UnicodeWidthStr;

        let (start, end) = selection.range();
        let mut selected = Vec::new();

        for (i, line) in self
            .lines
            .iter()
            .enumerate()
            .take(end.line + 1)
            .skip(start.line)
        {
            let left = if i == start.line { start.column } else { 0 };
            let right = if i == end.line {
                end.column
            } else {
                line.width()
            };
            let mut column = 0;
            let text: String = line
                .graphemes(true)
                .filter(|grapheme| {
                    let matches = column < right && column + grapheme.width() > left;
                    column += grapheme.width();
                    matches
                })
                .collect();
            selected.push(text);
        }

        selected.join("\n").trim_end().to_owned()
    }
}

#[derive(Default)]
pub struct Geometry {
    pub list_offsets: [usize; 4],
    pub details_scroll: u16,
    pub preview_scroll: u16,
    pub text_view: Option<TextView>,
    pub text_selection: Option<TextSelection>,
    pub help_scroll: u16,
}
