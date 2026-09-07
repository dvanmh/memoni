use anyhow::Result;
use egui::{Rect, scroll_area::State as ScrollAreaState};

pub trait ScrollAreaStateExt {
    fn reset_velocity(ctx: &egui::Context, scroll_area_id: egui::Id) -> Result<()>;
}

impl ScrollAreaStateExt for ScrollAreaState {
    fn reset_velocity(ctx: &egui::Context, scroll_area_id: egui::Id) -> Result<()> {
        // egui skips velocity when serializing scroll area's state
        // https://github.com/emilk/egui/blob/83e61c6fb064591e5cacb655156621f7eeafacc8/crates/egui/src/containers/scroll_area.rs#L43
        if let Some(state) = ScrollAreaState::load(ctx, scroll_area_id) {
            let bincode_config = bincode::config::standard();

            let encoded_state = bincode::serde::encode_to_vec(state, bincode_config)?;
            let velocity_reset_state: egui::scroll_area::State =
                bincode::serde::decode_from_slice(&encoded_state, bincode_config)?.0;

            velocity_reset_state.store(ctx, scroll_area_id);
        }

        Ok(())
    }
}

pub trait RectExt {
    fn flipped_y(self) -> Self;
}

impl RectExt for Rect {
    #[inline]
    fn flipped_y(self) -> Self {
        self.with_min_y(-self.max.y).with_max_y(-self.min.y)
    }
}
