//! This module contains the functionality for the following comment-related features
//! using the comment character defined in the user's `languages.toml`:
//! * toggle comments on lines over the selection
//! * continue comment when opening a new line

use crate::{chars, Change, Rope, RopeSlice, Selection, Tendril, Transaction};
use std::borrow::Cow;

/// Given text, a comment token, and a set of line indices, returns the following:
/// - Whether the given lines should be considered commented
///     - If any of the lines are uncommented, all lines are considered as such.
/// - The lines to change for toggling comments
///     - This is all provided lines excluding blanks lines.
/// - The column of the comment tokens
///     - Column of existing tokens, if the lines are commented; column to place tokens at otherwise.
/// - The margin to the right of the comment tokens
///     - Defaults to `1`. If any existing comment token is not followed by a space, changes to `0`.
fn find_line_comment(
    token: &str,
    text: RopeSlice,
    lines: impl IntoIterator<Item = usize>,
) -> (bool, Vec<usize>, usize, usize) {
    let mut commented = true;
    let mut to_change = Vec::new();
    let mut min = usize::MAX; // minimum col for find_first_non_whitespace_char
    let mut margin = 1;
    let token_len = token.chars().count();
    for line in lines {
        let line_slice = text.line(line);
        if let Some(pos) = chars::find_first_non_whitespace_char(line_slice) {
            let len = line_slice.len_chars();

            if pos < min {
                min = pos;
            }

            // line can be shorter than pos + token len
            let fragment = Cow::from(line_slice.slice(pos..std::cmp::min(pos + token.len(), len)));

            if fragment != token {
                // as soon as one of the non-blank lines doesn't have a comment, the whole block is
                // considered uncommented.
                commented = false;
            }

            // determine margin of 0 or 1 for uncommenting; if any comment token is not followed by a space,
            // a margin of 0 is used for all lines.
            if !matches!(line_slice.get_char(pos + token_len), Some(c) if c == ' ') {
                margin = 0;
            }

            // blank lines don't get pushed.
            to_change.push(line);
        }
    }
    (commented, to_change, min, margin)
}

#[must_use]
pub fn toggle_line_comments(doc: &Rope, selection: &Selection, token: Option<&str>) -> Transaction {
    let text = doc.slice(..);

    let token = token.unwrap_or("//");
    let comment = Tendril::from(format!("{} ", token));

    let mut lines: Vec<usize> = Vec::with_capacity(selection.len());

    let mut min_next_line = 0;
    for selection in selection {
        let (start, end) = selection.line_range(text);
        let start = start.clamp(min_next_line, text.len_lines());
        let end = (end + 1).min(text.len_lines());

        lines.extend(start..end);
        min_next_line = end;
    }

    let (commented, to_change, min, margin) = find_line_comment(token, text, lines);

    let mut changes: Vec<Change> = Vec::with_capacity(to_change.len());

    for line in to_change {
        let pos = text.line_to_char(line) + min;

        if !commented {
            // comment line
            changes.push((pos, pos, Some(comment.clone())));
        } else {
            // uncomment line
            changes.push((pos, pos + token.len() + margin, None));
        }
    }

    Transaction::change(doc, changes.into_iter())
}

/// Return the comment token of the current line if it is commented, along with the
/// position of the last character in the comment token.
/// Return None otherwise.
pub fn get_comment_token_and_position<'a>(
    doc: &Rope,
    line: usize,
    tokens: &'a [String],
) -> Option<(&'a str, usize)> {
    // TODO: don't continue shebangs
    if tokens.is_empty() {
        return None;
    }

    let mut result = None;
    let line_slice = doc.line(line);

    if let Some(pos) = chars::find_first_non_whitespace_char(line_slice) {
        let len = line_slice.len_chars();

        for token in tokens {
            // line can be shorter than pos + token length
            let fragment = Cow::from(line_slice.slice(pos..std::cmp::min(pos + token.len(), len)));

            if fragment == *token {
                // We don't necessarily want to break upon finding the first matching comment token
                // Instead, we check against all of the comment tokens and end up returning the longest
                // comment token that matches
                result = Some((token.as_str(), pos + token.len() - 1));
            }
        }
    }

    result
}

/// Determines whether the new line following the line at `line_idx` in
/// document should be prepended with a comment token.
pub fn handle_comment_continue<'a>(
    doc: &'a Rope,
    text: &'a mut String,
    line_idx: usize,
    comment_tokens: &'a [String],
) {
    if let Some((token, comment_token_ending_pos)) =
        get_comment_token_and_position(doc, line_idx, comment_tokens)
    {
        text.push_str(token);

        // find the position of the first non-whitespace char after the commet token so that
        // lines that continue a comment are indented to the same level as the previous line
        if let Some(trailing_whitespace) =
            chars::count_whitespace_after(doc.line(line_idx), comment_token_ending_pos)
        {
            let whitespace_to_insert = (0..=trailing_whitespace).map(|_| ' ').collect::<String>();

            text.push_str(&whitespace_to_insert);
        } else {
            text.push(' ');
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_toggle_line_comments() {
        // four lines, two space indented, except for line 1 which is blank.
        let mut doc = Rope::from("  1\n\n  2\n  3");
        // select whole document
        let mut selection = Selection::single(0, doc.len_chars() - 1);

        let text = doc.slice(..);

        let res = find_line_comment("//", text, 0..3);
        // (commented = true, to_change = [line 0, line 2], min = col 2, margin = 0)
        assert_eq!(res, (false, vec![0, 2], 2, 0));

        // comment
        let transaction = toggle_line_comments(&doc, &selection, None);
        transaction.apply(&mut doc);
        selection = selection.map(transaction.changes());

        assert_eq!(doc, "  // 1\n\n  // 2\n  // 3");

        // uncomment
        let transaction = toggle_line_comments(&doc, &selection, None);
        transaction.apply(&mut doc);
        selection = selection.map(transaction.changes());
        assert_eq!(doc, "  1\n\n  2\n  3");
        assert!(selection.len() == 1); // to ignore the selection unused warning

        // 0 margin comments
        doc = Rope::from("  //1\n\n  //2\n  //3");
        // reset the selection.
        selection = Selection::single(0, doc.len_chars() - 1);

        let transaction = toggle_line_comments(&doc, &selection, None);
        transaction.apply(&mut doc);
        selection = selection.map(transaction.changes());
        assert_eq!(doc, "  1\n\n  2\n  3");
        assert!(selection.len() == 1); // to ignore the selection unused warning

        // 0 margin comments, with no space
        doc = Rope::from("//");
        // reset the selection.
        selection = Selection::single(0, doc.len_chars() - 1);

        let transaction = toggle_line_comments(&doc, &selection, None);
        transaction.apply(&mut doc);
        selection = selection.map(transaction.changes());
        assert_eq!(doc, "");
        assert!(selection.len() == 1); // to ignore the selection unused warning

        // TODO: account for uncommenting with uneven comment indentation
    }

    #[test]
    fn test_get_comment_token_and_position() {
        let doc = Rope::from(
            "# 1\n    // 2    \n///3\n/// 4\n//! 5\n//! /// 6\n7 ///\n;8\n//////////// 9",
        );
        let tokens = vec![
            String::from("//"),
            String::from("///"),
            String::from("//!"),
            String::from(";"),
        ];

        assert_eq!(get_comment_token_and_position(&doc, 0, &tokens), None);
        assert_eq!(
            get_comment_token_and_position(&doc, 1, &tokens),
            Some(("//", 5))
        );
        assert_eq!(
            get_comment_token_and_position(&doc, 2, &tokens),
            Some(("///", 2))
        );
        assert_eq!(
            get_comment_token_and_position(&doc, 3, &tokens),
            Some(("///", 2))
        );
        assert_eq!(
            get_comment_token_and_position(&doc, 4, &tokens),
            Some(("//!", 2))
        );
        assert_eq!(
            get_comment_token_and_position(&doc, 5, &tokens),
            Some(("//!", 2))
        );
        assert_eq!(get_comment_token_and_position(&doc, 6, &tokens), None);
        assert_eq!(
            get_comment_token_and_position(&doc, 7, &tokens),
            Some((";", 0))
        );
        assert_eq!(
            get_comment_token_and_position(&doc, 8, &tokens),
            Some(("///", 2))
        );
    }

    #[test]
    fn test_handle_continue_comment() {
        let mut doc = Rope::from("// 1\n");
        let mut text = String::from(&doc);
        let comment_tokens = vec![String::from("//"), String::from("///")];

        handle_comment_continue(&doc, &mut text, 0, &comment_tokens);

        assert_eq!(text, String::from("// 1\n// "));

        doc = Rope::from("///2\n");
        text = String::from(&doc);

        handle_comment_continue(&doc, &mut text, 0, &comment_tokens);

        assert_eq!(text, String::from("///2\n/// "));

        doc = Rope::from("      // 3\n");
        text = String::from(&doc);

        handle_comment_continue(&doc, &mut text, 0, &comment_tokens);

        doc = Rope::from("///          4\n");
        text = String::from(&doc);

        handle_comment_continue(&doc, &mut text, 0, &comment_tokens);
    }
}
