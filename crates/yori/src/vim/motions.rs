//! Source-coordinate motions. Alignment spacers never become cursor positions.

use std::ops::Range;

use yori_document::{
    Document,
    editing::{self, TextSelection},
};

use super::keys::Motion;

pub(super) fn normal_cursor(document: &Document, offset: usize) -> usize {
    let last = document.lines().len().saturating_sub(1);
    let line = document.line_at_offset(offset).min(last);
    let content = document.line_content_range(line);

    if content.is_empty() {
        content.start
    } else {
        offset.clamp(
            content.start,
            editing::previous_grapheme(document.text(), content.end),
        )
    }
}

pub(super) fn first_word(document: &Document, offset: usize) -> usize {
    let content = document.line_content_range(document.line_at_offset(offset));

    document.text()[content.clone()]
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map_or(content.start, |(index, _)| content.start + index)
}

pub(super) fn line_range(document: &Document, start: usize, end: usize) -> Range<usize> {
    let first = document.line_at_offset(start.min(end));
    let last = document.line_at_offset(start.max(end));
    let start = document.line_content_range(first).start;
    let end = document
        .lines()
        .get(last)
        .map_or(document.text().len(), |line| line.full.end);

    start..end
}

pub(super) fn move_to(
    document: &Document,
    cursor: usize,
    motion: Motion,
    count: usize,
    column: &mut Option<usize>,
) -> usize {
    let text = document.text();
    let content = document.line_content_range(document.line_at_offset(cursor));
    let mut target = cursor;

    match motion {
        Motion::Left => {
            for _ in 0..count {
                target = editing::previous_grapheme(text, target).max(content.start);
            }
        }
        Motion::Right => {
            for _ in 0..count {
                target = editing::next_grapheme(text, target).min(content.end);
            }
        }
        Motion::Up | Motion::Down => {
            let direction = if motion == Motion::Up {
                editing::Motion::Up
            } else {
                editing::Motion::Down
            };

            for _ in 0..count.min(document.lines().len()) {
                target = editing::navigate(
                    document,
                    TextSelection::caret(target),
                    direction,
                    false,
                    column,
                )
                .head;
            }

            return target;
        }
        Motion::Word
        | Motion::BackWord
        | Motion::WordEnd
        | Motion::BigWord
        | Motion::BigBackWord
        | Motion::BigWordEnd => {
            for _ in 0..count {
                target = word(text, target, motion);
            }
        }
        Motion::Home => target = content.start,
        Motion::First => target = first_word(document, cursor),
        Motion::End => {
            let line = (document.line_at_offset(cursor) + count - 1)
                .min(document.lines().len().saturating_sub(1));
            target = document.line_content_range(line).end;
        }
        Motion::Line(line) => {
            let line = line.map_or(document.lines().len().saturating_sub(1), |line| {
                line.saturating_sub(1)
            });
            target = first_word(
                document,
                document
                    .line_content_range(line.min(document.lines().len().saturating_sub(1)))
                    .start,
            );
        }
    }

    *column = (motion == Motion::End).then_some(usize::MAX);
    target
}

// Vim's small words distinguish keyword characters, punctuation and whitespace;
// big WORDs only distinguish whitespace from everything else.
fn class(text: &str, offset: usize, big: bool) -> u8 {
    match text[offset..].chars().next() {
        None => 0,
        Some(ch) if ch.is_whitespace() => 0,
        Some(_) if big => 1,
        Some(ch) if ch.is_alphanumeric() || ch == '_' => 1,
        Some(_) => 2,
    }
}

fn is_big(motion: Motion) -> bool {
    matches!(
        motion,
        Motion::BigWord | Motion::BigBackWord | Motion::BigWordEnd
    )
}

fn word_end(big: bool) -> Motion {
    if big {
        Motion::BigWordEnd
    } else {
        Motion::WordEnd
    }
}

fn empty_line_at(text: &str, offset: usize) -> bool {
    let starts_line = offset == 0 || text.as_bytes().get(offset - 1) == Some(&b'\n');
    starts_line && matches!(text.as_bytes().get(offset), Some(b'\n' | b'\r'))
}

fn word(text: &str, cursor: usize, motion: Motion) -> usize {
    let big = is_big(motion);
    let class = |offset| class(text, offset, big);

    let mut at = cursor;
    if matches!(motion, Motion::BackWord | Motion::BigBackWord) {
        at = editing::previous_grapheme(text, at);
        while at > 0 && class(at) == 0 {
            if empty_line_at(text, at) {
                return at;
            }

            at = editing::previous_grapheme(text, at);
        }

        let kind = class(at);
        while at > 0 {
            let previous = editing::previous_grapheme(text, at);
            if class(previous) != kind {
                break;
            }

            at = previous;
        }

        return at;
    }

    if matches!(motion, Motion::WordEnd | Motion::BigWordEnd) {
        at = editing::next_grapheme(text, at);
        while at < text.len() && class(at) == 0 {
            at = editing::next_grapheme(text, at);
        }

        let kind = class(at);
        while at < text.len() {
            let next = editing::next_grapheme(text, at);
            if next == text.len() || class(next) != kind {
                break;
            }

            at = next;
        }

        return at;
    }

    let kind = class(at);
    if kind == 0 {
        at = editing::next_grapheme(text, at);
    } else {
        while at < text.len() && class(at) == kind {
            at = editing::next_grapheme(text, at);
        }
    }

    while at < text.len() && class(at) == 0 {
        if empty_line_at(text, at) {
            break;
        }

        at = editing::next_grapheme(text, at);
    }

    at
}

pub(super) fn inner_word(
    document: &Document,
    cursor: usize,
    count: usize,
    big: bool,
) -> Range<usize> {
    let text = document.text();
    let content = document.line_content_range(document.line_at_offset(cursor));
    let class = |offset| class(text, offset, big);

    let kind = class(cursor);
    let mut start = cursor;
    while start > content.start {
        let previous = editing::previous_grapheme(text, start);
        if class(previous) != kind {
            break;
        }

        start = previous;
    }

    let mut end = cursor;
    while end < content.end && class(end) == kind {
        end = editing::next_grapheme(text, end);
    }
    for _ in 1..count {
        let next = word(text, end, word_end(big));
        end = editing::next_grapheme(text, next);
    }

    start..end
}

pub(super) fn motion_range(
    document: &Document,
    cursor: usize,
    motion: Motion,
    count: usize,
    change: bool,
) -> (Range<usize>, bool) {
    // `cw` and `cW` change to the end of the word, like `ce` and `cE`.
    let word_motion = matches!(motion, Motion::Word | Motion::BigWord);
    if change && word_motion && class(document.text(), cursor, false) != 0 {
        let range = inner_word(document, cursor, 1, is_big(motion));
        let mut end = range.end;
        for _ in 1..count {
            end = editing::next_grapheme(
                document.text(),
                word(document.text(), end, word_end(is_big(motion))),
            );
        }

        return (cursor..end, false);
    }

    let target = move_to(document, cursor, motion, count, &mut None);
    let linewise = matches!(motion, Motion::Up | Motion::Down | Motion::Line(_));
    if linewise {
        return (line_range(document, cursor, target), true);
    }

    let mut range = cursor.min(target)..cursor.max(target);
    if matches!(motion, Motion::WordEnd | Motion::BigWordEnd) {
        range.end = editing::next_grapheme(document.text(), range.end);
    }

    // A final word's `dw` removes its trailing spaces, not the next line's indentation.
    if word_motion && document.line_at_offset(target) > document.line_at_offset(cursor) {
        let last_line = document.line_at_offset(target).saturating_sub(1);
        let preceding = document.line_content_range(last_line);
        if target <= first_word(document, target) && range.start < preceding.end {
            range.end = preceding.end;
        }
    }

    (range, false)
}
