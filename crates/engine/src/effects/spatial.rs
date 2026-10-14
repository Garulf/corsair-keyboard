use k100::{Frame, Rgb};

use crate::layer::{Effect, FrameCtx};

pub struct Solid {
    pub color: Rgb,
}

impl Effect for Solid {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        for geometry in ctx.layout.leds() {
            out[geometry.index as usize] = self.color;
        }
    }
}
