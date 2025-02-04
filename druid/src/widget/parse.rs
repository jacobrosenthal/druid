use std::fmt::Display;
use std::mem;
use std::str::FromStr;

use crate::kurbo::Size;
use crate::{
    BaseState, BoxConstraints, Data, Env, Event, EventCtx, LayoutCtx, PaintCtx, UpdateCtx, Widget,
};

/// Converts a `Widget<String>` to a `Widget<Option<T>>`, mapping parse errors to None
pub struct Parse<T> {
    widget: T,
    state: String,
}

impl<T> Parse<T> {
    pub fn new(widget: T) -> Self {
        Self {
            widget,
            state: String::new(),
        }
    }
}

impl<T: FromStr + Display + Data, W: Widget<String>> Widget<Option<T>> for Parse<W> {
    fn update(
        &mut self,
        ctx: &mut UpdateCtx,
        old_data: Option<&Option<T>>,
        data: &Option<T>,
        env: &Env,
    ) {
        let old = match *data {
            None => return, // Don't clobber the input
            Some(ref x) => mem::replace(&mut self.state, x.to_string()),
        };
        let old = old_data.map(|_| old);
        self.widget.update(ctx, old.as_ref(), &self.state, env)
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event, data: &mut Option<T>, env: &Env) {
        self.widget.event(ctx, event, &mut self.state, env);
        *data = self.state.parse().ok();
    }

    fn layout(
        &mut self,
        ctx: &mut LayoutCtx,
        bc: &BoxConstraints,
        _data: &Option<T>,
        env: &Env,
    ) -> Size {
        self.widget.layout(ctx, bc, &self.state, env)
    }

    fn paint(
        &mut self,
        paint: &mut PaintCtx,
        base_state: &BaseState,
        _data: &Option<T>,
        env: &Env,
    ) {
        self.widget.paint(paint, base_state, &self.state, env)
    }
}
