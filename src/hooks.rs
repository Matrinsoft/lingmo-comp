// SPDX-License-Identifier: GPL-3.0-only

use crate::shell::element::stack::{
    LingmoStackInternal, DefaultDecorations as DefaultStackDecorations, Message as StackMessage,
};
use crate::shell::element::window::{
    LingmoWindowInternal, DefaultDecorations as DefaultWindowDecorations, Message as WindowMessage,
};
use std::sync::{Arc, OnceLock};

/// An _unstable_ interface to customize lingmo-comp at compile-time by providing
/// hooks to be run in specific code paths.
#[derive(Default, Debug, Clone)]
pub struct Hooks {
    pub window_decorations:
        Option<Arc<dyn Decorations<LingmoWindowInternal, WindowMessage> + Send + Sync>>,
    pub stack_decorations:
        Option<Arc<dyn Decorations<LingmoStackInternal, StackMessage> + Send + Sync>>,
}

pub static HOOKS: OnceLock<Hooks> = OnceLock::new();

pub trait Decorations<Internal, Message>: std::fmt::Debug {
    fn view(&self, state: &Internal) -> Lingmo::Element<'_, Message>;
}

impl Decorations<LingmoWindowInternal, WindowMessage>
    for Option<Arc<dyn Decorations<LingmoWindowInternal, WindowMessage> + Send + Sync>>
{
    fn view(&self, window: &LingmoWindowInternal) -> Lingmo::Element<'_, WindowMessage> {
        match self {
            None => DefaultWindowDecorations.view(window),
            Some(deco) => deco.view(window),
        }
    }
}

impl Decorations<LingmoStackInternal, StackMessage>
    for Option<Arc<dyn Decorations<LingmoStackInternal, StackMessage> + Send + Sync>>
{
    fn view(&self, window: &LingmoStackInternal) -> Lingmo::Element<'_, StackMessage> {
        match self {
            None => DefaultStackDecorations.view(window),
            Some(deco) => deco.view(window),
        }
    }
}

