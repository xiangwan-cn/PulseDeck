//! Small, allocation-aware helpers for text that is about to cross into GTK.
//!
//! External commands and HTTP sources have a larger protocol budget than a
//! label, tooltip, or dialog should ever receive. Keeping the UI budget in a
//! shared module prevents one renderer from accidentally turning a bounded
//! source into an unbounded widget allocation.

pub const MAX_UI_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_UI_ERROR_BYTES: usize = 4 * 1024;
pub const MAX_UI_COLLECTION_ITEMS: usize = 512;

const TRUNCATION_MARKER: &str = "…（内容过长，已截断）";

/// Return at most `max_bytes` of `input`, ending on a UTF-8 boundary.
pub fn bounded_text(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_owned();
    }
    if max_bytes == 0 {
        return String::new();
    }

    let marker_len = TRUNCATION_MARKER.len();
    if max_bytes <= marker_len {
        return input[..char_boundary_at_or_before(input, max_bytes)].to_owned();
    }

    let prefix_budget = max_bytes - marker_len;
    let prefix_end = char_boundary_at_or_before(input, prefix_budget);
    let mut output = String::with_capacity(prefix_end + marker_len);
    output.push_str(&input[..prefix_end]);
    output.push_str(TRUNCATION_MARKER);
    output
}

/// Keep the end of a message, which is usually where a command's useful
/// error line lives, while enforcing the same UTF-8-safe byte budget.
pub fn bounded_tail(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_owned();
    }
    if max_bytes == 0 {
        return String::new();
    }

    let marker_len = TRUNCATION_MARKER.len();
    if max_bytes <= marker_len {
        let start = char_boundary_at_or_after(input, input.len() - max_bytes);
        return input[start..].to_owned();
    }

    let suffix_budget = max_bytes - marker_len;
    let start = char_boundary_at_or_after(input, input.len() - suffix_budget);
    let mut output = String::with_capacity(input.len() - start + marker_len);
    output.push_str(TRUNCATION_MARKER);
    output.push_str(&input[start..]);
    output
}

/// Keep the first `max_lines` lines (`0` means all available lines), then
/// apply the byte budget. The builder stops as soon as the budget is reached,
/// so a command that emits megabytes of one-line output is not copied again.
pub fn bounded_lines(input: &str, max_lines: usize, max_bytes: usize) -> String {
    if max_bytes == 0 || max_lines == 0 && input.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    let mut truncated = false;
    for (index, line) in input.lines().enumerate() {
        if max_lines != 0 && index >= max_lines {
            truncated = true;
            break;
        }
        let separator_len = usize::from(!output.is_empty());
        if output
            .len()
            .saturating_add(separator_len)
            .saturating_add(line.len())
            > max_bytes
        {
            truncated = true;
            break;
        }
        if separator_len != 0 {
            output.push('\n');
        }
        output.push_str(line);
    }

    if truncated {
        if output.is_empty() {
            input
                .lines()
                .next()
                .map_or_else(String::new, |line| bounded_text(line, max_bytes))
        } else {
            bounded_text(&format!("{output}\n{TRUNCATION_MARKER}"), max_bytes)
        }
    } else {
        output
    }
}

/// Build newline-separated UI rows without first materializing an unbounded
/// collection or repeatedly copying the accumulated output. The iterator is
/// stopped as soon as either the item or byte budget is reached.
pub fn bounded_joined_lines<I, S>(lines: I, max_lines: usize, max_bytes: usize) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if max_bytes == 0 {
        return String::new();
    }

    let mut output = String::new();
    let mut truncated = false;
    for (index, line) in lines.into_iter().enumerate() {
        if max_lines != 0 && index >= max_lines {
            truncated = true;
            break;
        }
        let line = line.as_ref();
        let separator_len = usize::from(!output.is_empty());
        let available = max_bytes.saturating_sub(output.len().saturating_add(separator_len));
        if line.len() > available {
            if separator_len != 0 {
                output.push('\n');
            }
            output.push_str(&bounded_text(line, available));
            truncated = true;
            break;
        }
        if separator_len != 0 {
            output.push('\n');
        }
        output.push_str(line);
    }

    if truncated {
        bounded_text(&format!("{output}\n{TRUNCATION_MARKER}"), max_bytes)
    } else {
        output
    }
}

fn char_boundary_at_or_before(input: &str, index: usize) -> usize {
    let mut index = index.min(input.len());
    while index > 0 && !input.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn char_boundary_at_or_after(input: &str, index: usize) -> usize {
    let mut index = index.min(input.len());
    while index < input.len() && !input.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::{bounded_lines, bounded_tail, bounded_text};

    #[test]
    fn truncation_respects_unicode_boundaries_and_budget() {
        let value = bounded_text("前缀-😀-后缀", 10);
        assert!(value.len() <= 10);
        assert!(std::str::from_utf8(value.as_bytes()).is_ok());
    }

    #[test]
    fn tail_keeps_the_end_of_long_errors() {
        let value = bounded_tail("开始\n最后一行", 12);
        assert!(value.len() <= 12);
        assert!(value.contains("最后") || value.contains("截断"));
    }

    #[test]
    fn lines_are_bounded_without_copying_all_input() {
        let value = bounded_lines("first\nsecond\nthird", 2, 64);
        assert!(value.starts_with("first\nsecond"));
        assert!(value.contains("截断"));
        let value = bounded_lines("first\nsecond\nthird", 0, 8);
        assert!(value.len() <= 8);
    }

    #[test]
    fn joined_lines_stop_at_item_and_byte_budgets() {
        let lines = (0..super::MAX_UI_COLLECTION_ITEMS + 8).map(|index| format!("item-{index}"));
        let value = super::bounded_joined_lines(lines, super::MAX_UI_COLLECTION_ITEMS, 128);
        assert!(value.len() <= 128);
        assert!(value.contains("截断") || value.lines().count() <= super::MAX_UI_COLLECTION_ITEMS);
    }
}
