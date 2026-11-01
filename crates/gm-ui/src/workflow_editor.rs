//! The editor of the workflow Script tab (D149, D151): the component's code editor with the Lua
//! language, the `ctx` completion of `script_completion`, and the editor's own text. This file is
//! the glue only: the rules of completion are in `script_completion`, and the check of the file
//! is the engine's.

use std::rc::Rc;
use std::sync::OnceLock;

use anyhow::Result;
use gm_core::script::surface::CtxSurface;
use gpui::{App, AppContext, Context, Entity, Subscription, Task, Window};
use gpui_component::input::{CompletionProvider, InputEvent, InputState, Position, Rope, TabSize};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Documentation, MarkupContent, MarkupKind, Range, TextEdit,
};

use crate::script_completion::{self, Item, ItemKind};
use crate::view::StatusView;

/// The functions of `ctx`, built once: the catalog does not change while the app runs.
fn surface() -> &'static CtxSurface {
    static SURFACE: OnceLock<CtxSurface> = OnceLock::new();
    SURFACE.get_or_init(CtxSurface::from_catalog)
}

/// The indentation of the templates: two spaces, no tabs.
const TABS: TabSize = TabSize {
    tab_size: 2,
    hard_tabs: false,
};

/// A new editor of `text`: Lua highlighting, line numbers, no soft wrap, and the completion of
/// `ctx`. The state is new for each document, so its undo history starts with the file.
pub(crate) fn new_editor(text: &str, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| {
        let mut state = InputState::new(window, cx)
            .code_editor("lua")
            .line_number(true)
            .soft_wrap(false)
            .tab_size(TABS)
            .default_value(text.to_owned());
        state.lsp.completion_provider = Some(Rc::new(ScriptCompletions));
        state
    })
}

/// Subscribes the view to the editor's changes, so each edit becomes the page's buffer.
pub(crate) fn subscribe(
    editor: &Entity<InputState>,
    window: &mut Window,
    cx: &mut Context<StatusView>,
) -> Subscription {
    cx.subscribe_in(
        editor,
        window,
        |this, _, event: &InputEvent, _window, cx| {
            if matches!(event, InputEvent::Change) {
                this.on_workflow_source_changed(cx);
            }
        },
    )
}

/// Offers what `script_completion` lists at the cursor, each replacing the word typed before it.
struct ScriptCompletions;

impl CompletionProvider for ScriptCompletions {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        let source = text.to_string();
        let items = script_completion::complete(&source, offset, surface());
        let word_start = source[..offset]
            .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
            .len();
        let range = Range::new(position(&source, word_start), position(&source, offset));
        // The typed word is what every item starts with, so it is the part the menu highlights.
        let typed = &source[word_start..offset];
        let items = items
            .into_iter()
            .map(|item| lsp_item(item, range, typed))
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        new_text
            .chars()
            .last()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || script_completion::is_trigger(c))
    }
}

/// The item in the component's terms: its icon, its one-line detail and its Markdown. The filter
/// text is the typed word: the menu highlights that many bytes of the label, and the label starts
/// with the word, so the highlight stays inside it.
fn lsp_item(item: Item, range: Range, typed: &str) -> CompletionItem {
    let kind = match item.kind {
        ItemKind::Function => CompletionItemKind::FUNCTION,
        ItemKind::Namespace => CompletionItemKind::MODULE,
        ItemKind::Field => CompletionItemKind::FIELD,
    };
    let documentation =
        (!item.documentation.is_empty()).then_some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: item.documentation,
        }));
    CompletionItem {
        label: item.label.clone(),
        kind: Some(kind),
        detail: Some(item.detail),
        filter_text: Some(typed.to_owned()),
        documentation,
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: item.label,
        })),
        ..CompletionItem::default()
    }
}

/// The editor's position of byte `offset` in `source`: its line, and its column in UTF-16 units
/// from the line's start, which is what the component's positions count.
fn position(source: &str, offset: usize) -> Position {
    let before = &source[..offset];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |at| at + 1);
    let column = source[line_start..offset].encode_utf16().count();
    Position::new(
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(column).unwrap_or(u32::MAX),
    )
}
