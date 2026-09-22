//! Native aligned editor: viewport rendering, syntax, selection, and input.

use std::{
    cell::{Cell, RefCell},
    ops::Range,
    path::PathBuf,
    rc::Rc,
};

mod change_navigation;
mod chrome;
mod completion;
mod connections;
mod footer;
mod highlighting;
mod input;
mod merge;
mod persistence;
mod restoration;
mod scrollbar;
mod vim;
mod whitespace;

use crate::appearance;
use yori_document::editing::{EditHistory, EditOutcome, Motion};

use gpui_kit::component::{ActiveTheme, ElementExt, highlighter::SyntaxHighlighter};
use gpui_kit::{
    App, Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, FocusHandle,
    Focusable, Font, HighlightStyle, InteractiveElement, IntoElement, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render, ScrollDelta, ScrollWheelEvent,
    SharedString, Styled, StyledText, TestSupportExt, TextRun, UTF16Selection, Window, canvas,
    container_query, div, point, px,
};
use ropey::Rope;
use yori::{
    display::{DisplayLine, max_display_columns, source_offset_at},
    document_info::{Language, LineEndings},
    geometry::{EditorGeometry, display_units, horizontal_scroll_limit, whole_rows},
    navigation::ChangeNavigation,
};
use yori_diff::{Alignment, DiffKind, IntralineDiff};
use yori_document::Document;

const FOOTER_HEIGHT: f32 = 32.0;
const HEADER_HEIGHT: f32 = 64.0;
const LINE_HEIGHT: f32 = 22.0;
const RESTORE_WIDTH: f32 = 26.0;
const TEXT_INSET: f32 = 8.0;
// Include the fixed inset in the gutter so shaping, caret and hit testing share one text origin.
const GUTTER_WIDTH: f32 = 64.0 + RESTORE_WIDTH + TEXT_INSET;
const TAB_WIDTH: usize = 4;
const OVERSCAN_ROWS: usize = 4;
pub(crate) const KEY_CONTEXT: &str = "AlignedEditor";

gpui_kit::actions!(
    aligned_editor,
    [
        CopySelected,
        CutSelected,
        Paste,
        RestoreSelectedLines,
        SelectAll,
        Undo,
        Redo,
        Backspace,
        Delete,
        Newline,
        InsertTab,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        MoveHome,
        MoveEnd,
        SelectHome,
        SelectEnd,
        MoveStart,
        MoveFinish,
        PreviousChange,
        NextChange,
        FocusPreviousPane,
        FocusNextPane,
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Incoming,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PaneFocusBoundary {
    Previous,
    Next,
}

#[derive(Clone, Debug)]
struct Selection {
    side: Side,
    anchor: usize,
    head: usize,
}

impl Selection {
    fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
}

#[derive(Clone, Copy)]
struct RowHighlighting<'a> {
    syntax: &'a highlighting::VisibleSyntax,
    intraline: &'a IntralineDiff,
}

pub(super) struct PaneDocument {
    path: PathBuf,
    editable: bool,
    saveable: bool,
    max_display_columns: usize,
    document: Document,
    highlighter: Option<SyntaxHighlighter>,
    highlight_generation: u64,
    syntax_cache: RefCell<highlighting::SyntaxCache>,
    language_override: Option<Language>,
    line_endings: LineEndings,
}

impl PaneDocument {
    pub(super) fn new(path: PathBuf, document: Document) -> Self {
        let max_display_columns = max_display_columns(&document, TAB_WIDTH);
        let line_endings = LineEndings::from_document(&document);

        Self {
            path,
            editable: false,
            saveable: false,
            max_display_columns,
            document,
            highlighter: None,
            highlight_generation: 0,
            syntax_cache: RefCell::default(),
            language_override: None,
            line_endings,
        }
    }

    #[cfg(test)]
    fn new_highlighted(path: PathBuf, document: Document) -> Self {
        let mut pane = Self::new(path, document);
        pane.highlighter = Self::highlighter_for(pane.language(), &pane.document);

        pane
    }

    fn language(&self) -> Language {
        self.language_override
            .unwrap_or_else(|| Language::detect(&self.path))
    }

    #[cfg(test)]
    fn highlighter_for(language: Language, document: &Document) -> Option<SyntaxHighlighter> {
        let grammar = highlighting::grammar_for(language)?;
        let mut highlighter = SyntaxHighlighter::new(grammar);
        highlighter.update(None, &Rope::from(document.text()), None);

        Some(highlighter)
    }

    fn change_language(&mut self, language: Option<Language>) -> bool {
        let previous = self.language();
        self.language_override = language;
        if self.language() == previous {
            return false;
        }

        self.highlighter = None;
        self.syntax_cache.get_mut().clear();

        true
    }

    #[cfg(test)]
    fn set_language(&mut self, language: Option<Language>) {
        if self.change_language(language) {
            self.highlighter = Self::highlighter_for(self.language(), &self.document);
        }
    }

    fn refresh_after_edit(&mut self, edit: &EditOutcome) {
        self.max_display_columns = max_display_columns(&self.document, TAB_WIDTH);
        self.syntax_cache.get_mut().clear();
        self.line_endings = LineEndings::from_document(&self.document);

        if let Some(highlighter) = &mut self.highlighter {
            let next = Rope::from(self.document.text());
            let position = |rope: &Rope, offset| {
                let row = rope.byte_to_line_idx(offset, ropey::LineType::LF);
                tree_sitter::Point::new(
                    row,
                    offset - rope.line_to_byte_idx(row, ropey::LineType::LF),
                )
            };

            let new_end = edit.replaced.start + edit.inserted_len;
            let change = tree_sitter::InputEdit {
                start_byte: edit.replaced.start,
                old_end_byte: edit.replaced.end,
                new_end_byte: new_end,
                start_position: position(highlighter.text(), edit.replaced.start),
                old_end_position: position(highlighter.text(), edit.replaced.end),
                new_end_position: position(&next, new_end),
            };

            highlighter.update(Some(change), &next, None);
        }
    }
}

pub(super) struct DirtyChanged;

struct DirtyState {
    original: String,
    modified: bool,
    saved_resolutions: Vec<bool>,
    saved_to_disk: bool,
}

impl DirtyState {
    fn new(text: &str) -> Self {
        Self {
            original: text.to_owned(),
            modified: false,
            saved_resolutions: Vec::new(),
            saved_to_disk: true,
        }
    }

    fn update(&mut self, text: &str) -> bool {
        let modified = text != self.original;
        let changed = modified != self.modified;
        self.modified = modified;

        changed
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VimKeybindings {
    Disabled,
    Enabled,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum HorizontalScrollbarVisibility {
    #[default]
    Hidden,
    Visible,
}

impl HorizontalScrollbarVisibility {
    fn height(self) -> f32 {
        match self {
            Self::Hidden => 0.0,
            Self::Visible => scrollbar::HEIGHT,
        }
    }

    fn is_visible(self) -> bool {
        self == Self::Visible
    }
}

impl From<bool> for VimKeybindings {
    fn from(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}

#[derive(Clone, Copy)]
enum EditorActivation {
    Focus,
    Preserve,
}

#[derive(Clone, Copy)]
enum InitialChange {
    First,
    Neutral,
}

#[derive(Clone, Copy)]
struct DiffOptions {
    editable: bool,
    saveable: bool,
    activation: EditorActivation,
    initial_change: InitialChange,
}

pub(super) struct AlignedEditor {
    left: PaneDocument,
    right: PaneDocument,
    alignment: Alignment,
    navigation: ChangeNavigation,
    history: EditHistory,
    merge: Option<merge::MergeState>,
    vim: yori::vim::Vim,
    vim_keybindings: VimKeybindings,
    dirty: DirtyState,
    saving: bool,
    preferred_column: Option<usize>,
    focus: FocusHandle,
    selection: Option<Selection>,
    vertical_scroll: f32,
    horizontal_scroll: f32,
    pending_initial_change_row: Option<usize>,
    show_whitespace: bool,
    show_connections: bool,
    hovered_connection: Option<Range<usize>>,
    scrollbar_grab: Option<f32>,
    horizontal_scrollbar_grab: Option<f32>,
    horizontal_scrollbar_visibility: HorizontalScrollbarVisibility,
    // Mouse events are window-local; measured bounds provide the editor's content-local inset.
    content_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl AlignedEditor {
    #[cfg(test)]
    pub(super) fn new(
        left: PaneDocument,
        right: PaneDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_diff(left, right, true, true, window, cx)
    }

    pub(super) fn new_diff(
        left: PaneDocument,
        right: PaneDocument,
        editable: bool,
        saveable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_diff_with_activation(left, right, editable, saveable, true, window, cx)
    }

    pub(super) fn new_review_diff(
        left: PaneDocument,
        right: PaneDocument,
        editable: bool,
        saveable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_diff_with_activation(left, right, editable, saveable, false, window, cx)
    }

    pub(super) fn new_merge_base(
        left: PaneDocument,
        right: PaneDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_diff_with_options(
            left,
            right,
            DiffOptions {
                editable: true,
                saveable: true,
                activation: EditorActivation::Focus,
                initial_change: InitialChange::Neutral,
            },
            window,
            cx,
        )
    }

    fn new_diff_with_activation(
        left: PaneDocument,
        right: PaneDocument,
        editable: bool,
        saveable: bool,
        activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_diff_with_options(
            left,
            right,
            DiffOptions {
                editable,
                saveable,
                activation: if activate {
                    EditorActivation::Focus
                } else {
                    EditorActivation::Preserve
                },
                initial_change: InitialChange::First,
            },
            window,
            cx,
        )
    }

    fn new_diff_with_options(
        mut left: PaneDocument,
        mut right: PaneDocument,
        options: DiffOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        left.editable = false;
        left.saveable = false;
        right.editable = options.editable;
        right.saveable = options.editable && options.saveable;

        let alignment = Alignment::between(&left.document, &right.document);
        let dirty = DirtyState::new(right.document.text());
        let config = crate::config::editor(cx);

        let focus = cx.focus_handle();
        if matches!(options.activation, EditorActivation::Focus) {
            focus.focus(window, cx);
        }
        cx.on_blur(&focus, window, |this, _, cx| {
            this.cancel_vim();
            cx.notify();
        })
        .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.scrollbar_grab = None;
                this.horizontal_scrollbar_grab = None;
                this.hovered_connection = None;
                this.cancel_vim();
                cx.notify();
            }
        })
        .detach();
        cx.observe_global::<crate::config::Configuration>(|this, cx| {
            let config = crate::config::editor(cx);
            if VimKeybindings::from(config.vim_keybindings) != this.vim_keybindings {
                this.cancel_vim();
                this.vim_keybindings = config.vim_keybindings.into();
                if config.vim_keybindings && this.selection.is_none() {
                    this.selection = Some(Selection {
                        side: Side::Right,
                        anchor: 0,
                        head: 0,
                    });
                }
            }
            if this.show_whitespace && !config.show_whitespace {
                this.horizontal_scroll = 0.0;
            }

            this.show_whitespace = config.show_whitespace;
            this.show_connections = config.show_change_connections;
            this.hovered_connection = None;
            cx.notify();
        })
        .detach();

        let mut editor = Self {
            left,
            right,
            alignment,
            navigation: ChangeNavigation::default(),
            history: EditHistory::default(),
            merge: None,
            vim: yori::vim::Vim::default(),
            vim_keybindings: config.vim_keybindings.into(),
            dirty,
            saving: false,
            preferred_column: None,
            focus,
            selection: None,
            vertical_scroll: 0.0,
            horizontal_scroll: 0.0,
            pending_initial_change_row: None,
            show_whitespace: config.show_whitespace,
            show_connections: config.show_change_connections,
            hovered_connection: None,
            scrollbar_grab: None,
            horizontal_scrollbar_grab: None,
            horizontal_scrollbar_visibility: HorizontalScrollbarVisibility::Hidden,
            content_bounds: Rc::new(Cell::new(Bounds::new(
                point(px(0.0), px(0.0)),
                window.viewport_size(),
            ))),
        };
        if matches!(options.initial_change, InitialChange::First) {
            editor.initialize_change_navigation();
        }
        editor.schedule_highlighting(Side::Left, window, cx);
        editor.schedule_highlighting(Side::Right, window, cx);

        editor
    }

    fn schedule_highlighting(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.document_mut(side);
        pane.highlight_generation = pane.highlight_generation.wrapping_add(1);
        pane.highlighter = None;
        pane.syntax_cache.get_mut().clear();

        let generation = pane.highlight_generation;
        let language = pane.language();
        let Some(grammar) = highlighting::grammar_for(language) else {
            return;
        };
        let text = pane.document.text().to_owned();
        let build = cx.background_executor().spawn(async move {
            let mut highlighter = SyntaxHighlighter::new(grammar);
            highlighter.update(None, &Rope::from(text.as_str()), None);

            (text, highlighter)
        });

        cx.spawn_in(window, async move |editor, cx| {
            let (text, highlighter) = build.await;
            let _ = editor.update_in(cx, |editor, _, cx| {
                let pane = editor.document_mut(side);
                if pane.highlight_generation != generation
                    || pane.language() != language
                    || pane.document.text() != text
                {
                    return;
                }

                pane.highlighter = Some(highlighter);
                pane.syntax_cache.get_mut().clear();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn can_edit(&self) -> bool {
        self.right.editable
    }

    pub(super) fn can_save(&self) -> bool {
        self.right.saveable
    }

    #[cfg(test)]
    pub(crate) fn highlighting_ready(&self) -> bool {
        let ready = |pane: &PaneDocument| {
            pane.highlighter.is_some() || highlighting::grammar_for(pane.language()).is_none()
        };

        ready(&self.left) && ready(&self.right)
    }

    #[cfg(test)]
    pub(crate) fn applied_preferences(&self) -> crate::config::EditorConfig {
        crate::config::EditorConfig {
            vim_keybindings: self.vim_keybindings == VimKeybindings::Enabled,
            show_whitespace: self.show_whitespace,
            show_change_connections: self.show_connections,
        }
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.dirty.modified
            || self.merge.as_ref().is_some_and(|merge| {
                merge
                    .session
                    .conflicts()
                    .iter()
                    .enumerate()
                    .any(|(index, conflict)| {
                        merge
                            .session
                            .state(conflict.id)
                            .is_some_and(|state| state.resolved)
                            != self
                                .dirty
                                .saved_resolutions
                                .get(index)
                                .copied()
                                .unwrap_or(false)
                    })
            })
    }

    pub(super) fn deactivate(&mut self, cx: &mut Context<Self>) {
        self.scrollbar_grab = None;
        self.horizontal_scrollbar_grab = None;
        self.hovered_connection = None;
        self.cancel_vim();
        self.finish_composition();
        cx.notify();
    }

    pub(crate) fn focus_leftmost_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_pane(Side::Left, window, cx);
    }

    fn active_side(&self) -> Side {
        self.selection
            .as_ref()
            .map_or(Side::Right, |selection| selection.side)
    }

    fn focus_previous_pane(
        &mut self,
        _: &FocusPreviousPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = match self.active_side() {
            Side::Left => {
                cx.emit(PaneFocusBoundary::Previous);
                return;
            }
            Side::Right => Side::Left,
            Side::Incoming => Side::Right,
        };

        self.focus_pane(target, window, cx);
    }

    fn focus_next_pane(&mut self, _: &FocusNextPane, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.active_side() {
            Side::Left => Side::Right,
            Side::Right if self.merge.is_some() => Side::Incoming,
            Side::Right | Side::Incoming => {
                cx.emit(PaneFocusBoundary::Next);
                return;
            }
        };

        self.focus_pane(target, window, cx);
    }

    fn focus_pane(&mut self, target: Side, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.active_side();
        let (row, x) = self.selection.as_ref().map_or((0, 0.0), |selection| {
            self.source_position(current, selection.head, window, cx)
        });
        let offset = self.source_offset_for_x(target, row, x, window, cx);

        self.cancel_vim();
        self.finish_composition();
        self.preferred_column = None;
        self.selection = Some(Selection {
            side: target,
            anchor: offset,
            head: offset,
        });
        self.sync_vim_selection(cx);
        self.reveal_cursor(window, cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn document(&self, side: Side) -> &PaneDocument {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
            Side::Incoming => {
                &self
                    .merge
                    .as_ref()
                    .expect("incoming pane requires merge mode")
                    .incoming
            }
        }
    }

    fn document_mut(&mut self, side: Side) -> &mut PaneDocument {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
            Side::Incoming => {
                &mut self
                    .merge
                    .as_mut()
                    .expect("incoming pane requires merge mode")
                    .incoming
            }
        }
    }

    fn line_for_row(&self, side: Side, row: usize) -> Option<usize> {
        if side == Side::Incoming {
            return self
                .merge
                .as_ref()?
                .display
                .rows()
                .get(row)?
                .sources
                .incoming;
        }

        self.alignment.rows().get(row).and_then(|row| {
            if side == Side::Left {
                row.left
            } else {
                row.right
            }
        })
    }

    fn pane_left(&self, side: Side) -> f32 {
        match side {
            Side::Left => 0.0,
            Side::Right => self.geometry().right_pane_left(),
            Side::Incoming => self.geometry().incoming_pane_left(),
        }
    }

    fn row_for_source(&self, side: Side, offset: usize) -> usize {
        if side != Side::Incoming {
            return self.alignment.row_for_offset(
                &self.document(side).document,
                offset,
                side == Side::Left,
            );
        }

        let document = &self.document(side).document;
        let line = document.line_at_offset(offset);
        self.merge
            .as_ref()
            .expect("incoming pane")
            .display
            .rows()
            .iter()
            .position(|row| row.sources.incoming == Some(line))
            .unwrap_or(self.alignment.rows().len())
    }

    fn source_offset_for(&self, side: Side, row: usize, display_byte: usize) -> usize {
        let document = &self.document(side).document;
        if side != Side::Incoming {
            return source_offset_at(
                &self.alignment,
                document,
                row,
                side == Side::Left,
                display_byte,
                TAB_WIDTH,
            );
        }

        if let Some(line) = self.line_for_row(side, row) {
            return DisplayLine::from_source(
                document.content(line),
                document.lines()[line].content.start,
                TAB_WIDTH,
            )
            .source_offset(display_byte);
        }
        self.merge
            .as_ref()
            .expect("incoming pane")
            .display
            .rows()
            .iter()
            .skip(row)
            .find_map(|row| row.sources.incoming)
            .map_or(document.text().len(), |line| {
                document.lines()[line].content.start
            })
    }

    fn source_offset_for_x(
        &self,
        side: Side,
        row: usize,
        x: f32,
        window: &mut Window,
        cx: &App,
    ) -> usize {
        let Some(line) = self.line_for_row(side, row) else {
            return self.source_offset_for(side, row, 0);
        };
        let document = &self.document(side).document;
        let source_line = &document.lines()[line];
        let display =
            DisplayLine::from_source(document.content(line), source_line.content.start, TAB_WIDTH);
        if x <= 0.0 || display.text.is_empty() {
            return self.source_offset_for(side, row, 0);
        }

        let run = TextRun {
            len: display.text.len(),
            font: Font {
                family: cx.theme().mono_font_family.clone(),
                ..Font::default()
            },
            color: cx.theme().foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(
            display.text.into(),
            cx.theme().mono_font_size,
            &[run],
            None,
        );
        let display_offset = shaped.closest_index_for_x(px(x));

        self.source_offset_for(side, row, display_offset)
    }

    fn geometry(&self) -> EditorGeometry {
        let bounds = self.content_bounds.get();
        let horizontal_scrollbar_height = self.horizontal_scrollbar_visibility.height();

        EditorGeometry::new(
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            (f32::from(bounds.size.width) - scrollbar::WIDTH).max(0.0),
            (f32::from(bounds.size.height) - FOOTER_HEIGHT - horizontal_scrollbar_height).max(0.0),
            HEADER_HEIGHT,
            GUTTER_WIDTH,
            LINE_HEIGHT,
        )
        .with_merge_layout(self.merge.is_some())
        .with_center_width(if self.show_connections {
            connections::WIDTH
        } else {
            0.0
        })
    }

    fn source_offset_at(
        &self,
        position: gpui_kit::Point<Pixels>,
        window: &mut Window,
        cx: &App,
    ) -> (Side, usize) {
        let hit = self.geometry().hit(
            f32::from(position.x),
            f32::from(position.y),
            self.vertical_scroll,
            self.horizontal_scroll,
        );
        let side = if hit.left_side {
            Side::Left
        } else if hit.incoming_side {
            Side::Incoming
        } else {
            Side::Right
        };
        let pane = self.document(side);
        let row = hit.row;
        let Some(line_index) = self.line_for_row(side, row) else {
            return (side, self.source_offset_for(side, row, 0));
        };

        let source_line = &pane.document.lines()[line_index];
        let display = DisplayLine::from_source(
            pane.document.content(line_index),
            source_line.content.start,
            TAB_WIDTH,
        );

        if hit.text_x <= 0.0 || display.text.is_empty() {
            return (side, self.source_offset_for(side, row, 0));
        }

        let theme = cx.theme();
        let run = TextRun {
            len: display.text.len(),
            font: Font {
                family: theme.mono_font_family.clone(),
                ..Font::default()
            },
            color: theme.foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(
            display.text.clone().into(),
            theme.mono_font_size,
            &[run],
            None,
        );
        let display_offset = shaped.closest_index_for_x(px(hit.text_x));

        (side, self.source_offset_for(side, row, display_offset))
    }

    fn max_horizontal_scroll(&self, window: &mut Window, cx: &App) -> f32 {
        let theme = cx.theme();
        let run = TextRun {
            len: 1,
            font: Font {
                family: theme.mono_font_family.clone(),
                ..Font::default()
            },
            color: theme.foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let cell_width = f32::from(
            window
                .text_system()
                .shape_line(" ".into(), theme.mono_font_size, &[run], None)
                .width(),
        );

        let max_columns = self
            .left
            .max_display_columns
            .max(self.right.max_display_columns)
            .max(self.merge.as_ref().map_or(0, |merge| {
                merge.incoming.max_display_columns.max(if merge.show_base {
                    merge.base_columns
                } else {
                    0
                })
            }));
        let marker_columns = if self.show_whitespace {
            whitespace::ENDING_LABEL_COLUMNS
        } else {
            0
        };

        horizontal_scroll_limit(
            max_columns + marker_columns,
            cell_width,
            self.geometry().text_viewport_width() - 2.0,
        )
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let hit = self.geometry().hit(
            f32::from(event.position.x),
            f32::from(event.position.y),
            self.vertical_scroll,
            self.horizontal_scroll,
        );
        if self
            .merge
            .as_ref()
            .and_then(|merge| merge.display.rows().get(hit.row))
            .is_some_and(|row| !matches!(row.kind, merge::RowKind::Aligned))
        {
            return;
        }

        self.reposition_vim();
        self.finish_composition();
        self.preferred_column = None;

        self.focus.focus(window, cx);

        let (side, offset) = self.source_offset_at(event.position, window, cx);
        if side != Side::Right {
            self.cancel_vim();
        }

        let anchor = self
            .selection
            .as_ref()
            .filter(|selection| event.modifiers.shift && selection.side == side)
            .map_or(offset, |selection| selection.anchor);

        self.selection = Some(Selection {
            side,
            anchor,
            head: offset,
        });
        self.sync_vim_selection(cx);
        self.locate_pointer_change(event.position);

        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !event.dragging() {
            return;
        }

        let (side, offset) = self.source_offset_at(event.position, window, cx);
        if let Some(selection) = &mut self.selection
            && selection.side == side
        {
            selection.head = offset;
            self.sync_vim_selection(cx);
            self.locate_pointer_change(event.position);

            cx.notify();
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
            ScrollDelta::Lines(delta) => (delta.x * LINE_HEIGHT, delta.y * LINE_HEIGHT),
        };
        let horizontal = if event.shift {
            delta.0 + delta.1
        } else {
            delta.0
        };
        let vertical = if event.shift { 0.0 } else { delta.1 };

        let max_vertical = self
            .geometry()
            .vertical_scroll_limit(self.alignment.rows().len());
        self.vertical_scroll = (self.vertical_scroll - vertical).clamp(0.0, max_vertical);

        let max_horizontal = self.max_horizontal_scroll(window, cx);
        self.horizontal_scroll = (self.horizontal_scroll - horizontal).clamp(0.0, max_horizontal);

        cx.notify();
        cx.stop_propagation();
    }

    fn copy_selected(&mut self, _: &CopySelected, _: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = &self.selection else {
            return;
        };
        let range = selection.range();
        if range.is_empty() {
            return;
        }

        let text = self
            .document(selection.side)
            .document
            .copy_range(range)
            .to_owned();

        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn visible_source_lines(
        &self,
        side: Side,
        display_rows: Range<usize>,
    ) -> Option<std::ops::RangeInclusive<usize>> {
        let mut lines = display_rows.filter_map(|row| self.line_for_row(side, row));
        let first = lines.next()?;
        let last = lines.last().unwrap_or(first);

        Some(first..=last)
    }

    fn text_highlights(
        &self,
        side: Side,
        pane: &PaneDocument,
        line_index: usize,
        display: &DisplayLine,
        highlighting: RowHighlighting<'_>,
        cx: &Context<Self>,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let source_range = pane.document.lines()[line_index].content.clone();
        let syntax = highlighting.syntax.line(line_index);

        let selected = self.selection.as_ref().and_then(|selection| {
            if selection.side != side {
                return None;
            }

            let range = selection.range();
            let overlap = range.start.max(source_range.start)..range.end.min(source_range.end);
            (overlap.start < overlap.end).then(|| display.display_range(overlap))
        });

        let marked = (side == Side::Right)
            .then(|| self.marked_range())
            .flatten()
            .map(|range| display.display_range(range))
            .filter(|range| !range.is_empty());

        let changed: Vec<_> = match side {
            Side::Left => &highlighting.intraline.left,
            Side::Right | Side::Incoming => &highlighting.intraline.right,
        }
        .iter()
        .map(|range| display.display_range(range.clone()))
        .filter(|range| !range.is_empty())
        .collect();

        highlighting::compose(
            display.text.len(),
            syntax,
            &changed,
            selected.as_ref(),
            marked.as_ref(),
            &highlighting::OverlayColors {
                changed: match side {
                    Side::Left => appearance::removed().emphasis,
                    Side::Right | Side::Incoming => appearance::added().emphasis,
                },
                selected: cx.theme().selection,
                foreground: cx.theme().foreground,
            },
        )
    }

    fn row_colors(kind: DiffKind, side: Side) -> Option<appearance::DiffColors> {
        match (kind, side) {
            (DiffKind::Removed | DiffKind::Modified, Side::Left) => Some(appearance::removed()),
            (DiffKind::Added | DiffKind::Modified, Side::Right | Side::Incoming) => {
                Some(appearance::added())
            }
            _ => None,
        }
    }

    fn render_line_number(line_index: usize, color: gpui_kit::Hsla) -> impl IntoElement {
        div()
            .absolute()
            .left(px(RESTORE_WIDTH))
            .w(px(GUTTER_WIDTH - RESTORE_WIDTH - TEXT_INSET))
            .h(px(LINE_HEIGHT))
            .overflow_hidden()
            .text_right()
            .pr(px(8.0))
            .text_color(color)
            .child((line_index + 1).to_string())
    }

    fn render_pane_row(
        &self,
        side: Side,
        row_index: usize,
        top: f32,
        geometry: EditorGeometry,
        highlighting: RowHighlighting<'_>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pane_width = geometry.pane_width();
        let text_viewport_width = geometry.text_viewport_width();

        let row = &self.alignment.rows()[row_index];
        let pane = self.document(side);
        let line = self.line_for_row(side, row_index);
        let kind = if side == Side::Incoming {
            let result = row.right.map(|line| self.right.document.full_line(line));
            let incoming = line.map(|line| pane.document.full_line(line));
            if result == incoming {
                DiffKind::Equal
            } else {
                DiffKind::Added
            }
        } else {
            row.kind
        };
        let colors = Self::row_colors(kind, side);
        let background = if line.is_none() {
            appearance::gap()
        } else {
            colors
                .as_ref()
                .map_or(cx.theme().background, |colors| colors.line)
        };

        let mut container = div()
            .absolute()
            .top(px(top))
            .left(px(self.pane_left(side)))
            .w(px(pane_width))
            .h(px(LINE_HEIGHT))
            .overflow_hidden()
            .bg(background);

        if let Some(line_index) = line {
            let source_line = &pane.document.lines()[line_index];
            let display = DisplayLine::from_source(
                pane.document.content(line_index),
                source_line.content.start,
                TAB_WIDTH,
            );
            let highlights =
                self.text_highlights(side, pane, line_index, &display, highlighting, cx);
            let whitespace = self.show_whitespace.then(|| {
                self.render_whitespace(
                    &display,
                    pane.document.content(line_index),
                    source_line.ending,
                    cx,
                )
            });
            let text =
                StyledText::new(SharedString::from(display.text)).with_highlights(highlights);

            container = container
                .child(Self::render_line_number(
                    line_index,
                    colors
                        .as_ref()
                        .map_or(cx.theme().muted_foreground, |colors| colors.marker),
                ))
                .child(
                    div()
                        .absolute()
                        .left(px(GUTTER_WIDTH))
                        .w(px(text_viewport_width))
                        .h(px(LINE_HEIGHT))
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .left(px(-self.horizontal_scroll))
                                .whitespace_nowrap()
                                .child(text),
                        )
                        .children(whitespace),
                );
        }

        let current = self
            .navigation
            .current(&self.alignment)
            .is_some_and(|index| self.alignment.blocks()[index].rows.contains(&row_index));
        let marker = if current {
            Some(cx.theme().primary)
        } else {
            colors.map(|colors| colors.marker)
        };

        if let Some(marker) = marker {
            container = container.child(
                div()
                    .absolute()
                    .left(px(GUTTER_WIDTH - TEXT_INSET - 3.0))
                    .top(px(0.0))
                    .w(px(if current { 3.0 } else { 2.0 }))
                    .h(px(LINE_HEIGHT))
                    .bg(marker),
            );
        }

        container
    }
}

impl gpui_kit::EventEmitter<DirtyChanged> for AlignedEditor {}
impl gpui_kit::EventEmitter<PaneFocusBoundary> for AlignedEditor {}

#[cfg(test)]
impl AlignedEditor {
    pub(crate) fn active_pane_index(&self) -> usize {
        match self.active_side() {
            Side::Left => 0,
            Side::Right => 1,
            Side::Incoming => 2,
        }
    }
}

impl Focusable for AlignedEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AlignedEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = cx.entity();

        // Build viewport-dependent content only after this frame's layout has
        // assigned its size, including when a previously hidden tab is activated.
        container_query(move |size, window, cx| {
            editor.update(cx, |editor, cx| {
                let mut bounds = editor.content_bounds.get();
                bounds.size = size;
                editor.content_bounds.set(bounds);
                editor.resolve_initial_change_viewport();

                editor.render_content(window, cx)
            })
        })
    }
}

impl AlignedEditor {
    #[expect(
        clippy::too_many_lines,
        reason = "one declarative GPUI widget tree keeps input bindings, clipping, and overlays in their rendering order"
    )]
    fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let max_horizontal_scroll = self.max_horizontal_scroll(window, cx);
        self.horizontal_scrollbar_visibility = if max_horizontal_scroll > 0.0 {
            HorizontalScrollbarVisibility::Visible
        } else {
            self.horizontal_scrollbar_grab = None;
            HorizontalScrollbarVisibility::Hidden
        };

        let geometry = self.geometry();
        let width = geometry.content_width();
        let pane_width = geometry.pane_width();
        let text_viewport_width = geometry.text_viewport_width();

        self.horizontal_scroll = self.horizontal_scroll.min(max_horizontal_scroll);
        self.vertical_scroll = self
            .vertical_scroll
            .min(geometry.vertical_scroll_limit(self.alignment.rows().len()));

        let first_row = whole_rows(self.vertical_scroll / LINE_HEIGHT);
        let row_offset = self.vertical_scroll % LINE_HEIGHT;
        let visible_count =
            whole_rows((geometry.rows_viewport_height() / LINE_HEIGHT).ceil()) + OVERSCAN_ROWS;
        let end_row = (first_row + visible_count).min(self.alignment.rows().len());
        let visible_rows = first_row..end_row;
        let theme = &cx.theme().highlight_theme;
        let left_syntax = self.left.syntax_cache.borrow_mut().visible(
            self.left.highlighter.as_ref(),
            &self.left.document,
            self.visible_source_lines(Side::Left, visible_rows.clone()),
            theme,
            TAB_WIDTH,
        );
        let right_syntax = self.right.syntax_cache.borrow_mut().visible(
            self.right.highlighter.as_ref(),
            &self.right.document,
            self.visible_source_lines(Side::Right, visible_rows.clone()),
            theme,
            TAB_WIDTH,
        );
        let incoming_syntax = self.merge.as_ref().map_or_else(Default::default, |merge| {
            merge.incoming.syntax_cache.borrow_mut().visible(
                merge.incoming.highlighter.as_ref(),
                &merge.incoming.document,
                self.visible_source_lines(Side::Incoming, visible_rows.clone()),
                theme,
                TAB_WIDTH,
            )
        });

        let mut rows = div()
            .id("rows-viewport")
            .test_support()
            .absolute()
            .top(px(HEADER_HEIGHT))
            .left(px(0.0))
            .w(px(width))
            .h(px(geometry.rows_viewport_height()))
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move));

        for row_index in first_row..end_row {
            let top = geometry.visible_row_top(row_index, first_row, row_offset);
            match self
                .merge
                .as_ref()
                .map(|merge| &merge.display.rows()[row_index].kind)
            {
                Some(merge::RowKind::ConflictHeader(id)) => {
                    rows = rows.child(self.render_merge_header(*id, top, geometry, cx));
                    continue;
                }
                Some(merge::RowKind::Base(base)) => {
                    rows = rows.child(self.render_base_preview_row(base, top, geometry, cx));
                    continue;
                }
                Some(merge::RowKind::Aligned) | None => {}
            }

            // Fine-grained work is viewport-only and shared by both cells.
            let intraline =
                self.alignment
                    .intraline(&self.left.document, &self.right.document, row_index);

            rows = rows
                .child(self.render_pane_row(
                    Side::Left,
                    row_index,
                    top,
                    geometry,
                    RowHighlighting {
                        syntax: &left_syntax,
                        intraline: &intraline,
                    },
                    cx,
                ))
                .child(self.render_pane_row(
                    Side::Right,
                    row_index,
                    top,
                    geometry,
                    RowHighlighting {
                        syntax: &right_syntax,
                        intraline: &intraline,
                    },
                    cx,
                ));
            if self.merge.is_some() {
                let result = self
                    .line_for_row(Side::Right, row_index)
                    .map_or("", |line| self.right.document.content(line));
                let incoming = self
                    .line_for_row(Side::Incoming, row_index)
                    .map_or("", |line| {
                        self.document(Side::Incoming).document.content(line)
                    });
                let intraline = IntralineDiff::between(result, incoming);
                rows = rows.child(self.render_pane_row(
                    Side::Incoming,
                    row_index,
                    top,
                    geometry,
                    RowHighlighting {
                        syntax: &incoming_syntax,
                        intraline: &intraline,
                    },
                    cx,
                ));
            }
        }

        if self.focus.is_focused(window)
            && let Some(selection) = &self.selection
        {
            let cursor = if Self::vim_enabled(cx) {
                self.vim.cursor(yori_document::editing::TextSelection {
                    anchor: selection.anchor,
                    head: selection.head,
                })
            } else {
                selection.head
            };
            let (row, x) = self.cursor_position(cursor, window, cx);
            let modal_cursor = Self::vim_enabled(cx) && self.vim.mode() != yori::vim::Mode::Insert;
            let caret_width = if modal_cursor {
                let next = yori_document::editing::next_grapheme(
                    self.document(selection.side).document.text(),
                    cursor,
                );
                let (next_row, next_x) = self.cursor_position(next, window, cx);
                if row == next_row {
                    (next_x - x).max(8.0)
                } else {
                    8.0
                }
            } else {
                1.0
            };
            let caret_color = if modal_cursor {
                cx.theme().foreground.opacity(0.35)
            } else {
                cx.theme().foreground
            };

            rows = rows.child(
                div()
                    .absolute()
                    .left(px(self.pane_left(selection.side) + GUTTER_WIDTH))
                    .top(px(0.0))
                    .w(px(text_viewport_width))
                    .h(px(geometry.rows_viewport_height()))
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .left(px(x - self.horizontal_scroll))
                            .top(px(display_units(row) * LINE_HEIGHT - self.vertical_scroll))
                            .w(px(caret_width))
                            .h(px(LINE_HEIGHT))
                            .bg(caret_color),
                    ),
            );
        }

        if self.merge.is_none() {
            rows = rows.child(self.render_restore_controls(geometry, cx));
        } else {
            rows = rows.child(self.render_merge_conflict_controls(geometry, cx));
        }

        let input_entity = cx.entity();
        let input_focus = self.focus.clone();
        let measured_content_bounds = Rc::clone(&self.content_bounds);

        div()
            .id("aligned-editor")
            .test_support()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .cursor_text()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(cx.theme().mono_font_size)
            .line_height(px(LINE_HEIGHT))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .capture_key_down(cx.listener(Self::vim_key))
            .on_action(cx.listener(Self::previous_change))
            .on_action(cx.listener(Self::next_change))
            .on_action(cx.listener(Self::focus_previous_pane))
            .on_action(cx.listener(Self::focus_next_pane))
            .on_action(cx.listener(Self::restore_selected_lines))
            .on_action(cx.listener(Self::copy_selected))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::insert_tab))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(|this, _: &MoveLeft, w, cx| {
                this.move_cursor(Motion::Left, false, w, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveRight, w, cx| {
                this.move_cursor(Motion::Right, false, w, cx);
            }))
            .on_action(
                cx.listener(|this, _: &MoveUp, w, cx| this.move_cursor(Motion::Up, false, w, cx)),
            )
            .on_action(cx.listener(|this, _: &MoveDown, w, cx| {
                this.move_cursor(Motion::Down, false, w, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, w, cx| {
                this.move_cursor(Motion::Left, true, w, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectRight, w, cx| {
                this.move_cursor(Motion::Right, true, w, cx);
            }))
            .on_action(
                cx.listener(|this, _: &SelectUp, w, cx| this.move_cursor(Motion::Up, true, w, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectDown, w, cx| {
                this.move_cursor(Motion::Down, true, w, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveHome, w, cx| {
                this.move_cursor(Motion::Home, false, w, cx);
            }))
            .on_action(
                cx.listener(|this, _: &MoveEnd, w, cx| this.move_cursor(Motion::End, false, w, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectHome, w, cx| {
                this.move_cursor(Motion::Home, true, w, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectEnd, w, cx| {
                this.move_cursor(Motion::End, true, w, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveStart, w, cx| {
                this.move_cursor(Motion::Start, false, w, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveFinish, w, cx| {
                this.move_cursor(Motion::Finish, false, w, cx);
            }))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_prepaint(move |bounds, _, _| {
                // Input uses the same frame's window-local origin and size.
                measured_content_bounds.set(bounds);
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        if input_entity.read(cx).right_selection().is_some()
                            && input_entity.read(cx).accepts_text(cx)
                        {
                            window.handle_input(
                                &input_focus,
                                ElementInputHandler::new(bounds, input_entity.clone()),
                                cx,
                            );
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(rows)
            .child(self.render_scrollbar(cx))
            .children(
                self.horizontal_scrollbar_visibility
                    .is_visible()
                    .then(|| self.render_horizontal_scrollbar(max_horizontal_scroll, cx)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(pane_width))
                    .w(px(geometry.center_width()))
                    .h(px(HEADER_HEIGHT))
                    .bg(cx.theme().secondary)
                    .border_b_1()
                    .border_color(cx.theme().border),
            )
            .child(self.render_footer(pane_width, cx))
            .child(self.render_pane_header(Side::Left, pane_width, cx))
            .child(self.render_pane_header(Side::Right, pane_width, cx))
            .children(
                self.merge
                    .as_ref()
                    .map(|_| self.render_pane_header(Side::Incoming, pane_width, cx)),
            )
            .children((1..if self.merge.is_some() { 3 } else { 2 }).map(|column| {
                div()
                    .absolute()
                    .top_0()
                    .left(px(display_units(column) * pane_width))
                    .w(px(1.0))
                    .h(px(HEADER_HEIGHT + geometry.rows_viewport_height()))
                    .bg(cx.theme().border)
            }))
    }
}

pub(crate) fn fixed_key_bindings() -> Vec<KeyBinding> {
    input::fixed_key_bindings()
}

pub(super) fn init(cx: &mut App) {
    crate::config::init_transient(cx);
    vim::init(cx);
    crate::keymap::init(cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext, component::highlighter::HighlightTheme};
    use yori_document::editing::TextSelection;

    fn pane(text: &str) -> PaneDocument {
        PaneDocument::new_highlighted(
            PathBuf::from("fixture.rs"),
            Document::from_bytes(text.as_bytes().to_vec()).unwrap(),
        )
    }

    #[gpui_kit::test]
    fn background_highlighting_cannot_overwrite_a_replaced_pane(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::appearance::init(cx);
            crate::editor::init(cx);
        });

        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                AlignedEditor::new(
                    PaneDocument::new(
                        "baseline.rs".into(),
                        Document::from_bytes(b"fn baseline() {}\n".to_vec()).unwrap(),
                    ),
                    PaneDocument::new(
                        "current.rs".into(),
                        Document::from_bytes(b"fn old() {}\n".to_vec()).unwrap(),
                    ),
                    window,
                    cx,
                )
            });
            editor = Some(view.clone());

            gpui_kit::component::Root::new(view, window, cx)
        });
        let editor = editor.unwrap();

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.right = PaneDocument::new(
                    "current.rs".into(),
                    Document::from_bytes(b"fn replacement() {}\n".to_vec()).unwrap(),
                );
                editor.schedule_highlighting(Side::Right, window, cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            let editor = editor.read(cx);
            assert_eq!(editor.right.document.text(), "fn replacement() {}\n");
            assert_eq!(
                editor
                    .right
                    .highlighter
                    .as_ref()
                    .unwrap()
                    .text()
                    .to_string(),
                editor.right.document.text()
            );
        });
    }

    #[test]
    fn dirty_state_tracks_loaded_local_bytes_not_the_comparison_baseline() {
        let mut document = Document::from_bytes(b"local\r\n".to_vec()).unwrap();
        let mut dirty = DirtyState::new(document.text());
        let mut history = EditHistory::default();
        assert!(!dirty.modified);

        let edit = history
            .replace(&mut document, TextSelection::caret(0), 0..5, "baseline")
            .unwrap();
        assert!(dirty.update(document.text()));
        assert!(dirty.modified);
        assert!(!dirty.update(document.text()));

        let undo = history
            .undo(&mut document, edit.selection)
            .unwrap()
            .unwrap();
        assert!(dirty.update(document.text()));
        assert!(!dirty.modified);

        history.redo(&mut document, undo.selection).unwrap();
        assert!(dirty.update(document.text()));
        assert!(dirty.modified);
    }

    #[test]
    fn syntax_refresh_matches_a_fresh_parse_after_edit_and_undo() {
        let mut pane = pane("fn greet() {\n    let name = \"hello\";\n}\n");
        let original = pane.document.text().to_owned();
        let mut history = EditHistory::default();
        let offset = original.find("hello").unwrap();
        let selection = TextSelection {
            anchor: offset,
            head: offset + 5,
        };

        let edit = history
            .replace(
                &mut pane.document,
                selection,
                selection.range(),
                "界\\nworld",
            )
            .unwrap();
        pane.refresh_after_edit(&edit);

        let fresh = PaneDocument::new_highlighted(pane.path.clone(), pane.document.clone());
        let theme = HighlightTheme::default_dark();
        let range = 0..pane.document.text().len();
        let expected = fresh
            .highlighter
            .as_ref()
            .unwrap()
            .styles(&range, theme.as_ref());

        assert!(!expected.is_empty());
        assert_eq!(
            pane.highlighter
                .as_ref()
                .unwrap()
                .styles(&range, theme.as_ref()),
            expected
        );

        let undo = history
            .undo(&mut pane.document, edit.selection)
            .unwrap()
            .unwrap();
        pane.refresh_after_edit(&undo);

        assert_eq!(pane.document.text(), original);
        assert_eq!(
            pane.highlighter.as_ref().unwrap().text().to_string(),
            original
        );
    }

    #[test]
    #[ignore = "explicit 5k editing latency measurement"]
    fn five_thousand_line_edit_refresh_and_realign() {
        use std::fmt::Write as _;

        let mut source = String::new();
        for line in 0..5_000 {
            writeln!(source, "fn row_{line}() -> usize {{ {line} }}").unwrap();
        }

        let left = Document::from_bytes(source.as_bytes().to_vec()).unwrap();
        let mut pane = pane(&source);
        let mut history = EditHistory::default();

        let started = std::time::Instant::now();
        for _ in 0..10 {
            let edit = history
                .replace(
                    &mut pane.document,
                    TextSelection::caret(0),
                    0..0,
                    "// note\n",
                )
                .unwrap();
            pane.refresh_after_edit(&edit);

            let alignment = Alignment::between(&left, &pane.document);
            assert_eq!(alignment.rows().len(), 5_001);

            let undo = history
                .undo(&mut pane.document, edit.selection)
                .unwrap()
                .unwrap();
            pane.refresh_after_edit(&undo);

            assert_eq!(
                Alignment::between(&left, &pane.document).rows().len(),
                5_000
            );
        }

        eprintln!(
            "5k: 20 edit/syntax/realignment operations in {:?}",
            started.elapsed()
        );
        assert_eq!(pane.document.text(), source);
    }
}
