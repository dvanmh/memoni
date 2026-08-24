pub mod color;
pub mod config;
pub mod ext;
pub mod freedesktop_cache;
pub mod input;
pub mod keymap_action;
pub mod opengl_context;
pub mod ordered_hash_map;
pub mod persistence;
pub mod selection;
pub mod selection_item;
pub mod timerfd_source;
pub mod transfer_window_pool;
pub mod ui;
pub mod utils;
pub mod widgets;
pub mod x11_key_converter;
pub mod x11_window;

pub use ext::ScrollAreaStateExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppMode {
    Normal,
    Help,
}
