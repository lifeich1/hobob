use bevy::prelude::*;
use std::convert::TryInto;

pub struct ScrollWidgetsPlugin();

impl Plugin for ScrollWidgetsPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_system(scroll_sim.system());
    }
}

/// Scroll progression in [0, 100]
///
/// To watch the progression, attach this component on widgets.
#[derive(Default)]
pub struct ScrollProgression(pub usize);

/// A scroll-simulate list widget.
///
/// The widget will automatically control children's visibilities to simulate scroll.
///
/// Because nowadays children structure cannot do actual removing without rebuild the whold tree,
/// it is unable to support remove item now. But maybe support mark-removed in future.
pub struct ScrollSimListWidget {
    show_limit: usize,
    current_step: usize,
    step_move: i32,
    invalidate: bool,
}

impl Default for ScrollSimListWidget {
    fn default() -> Self {
        Self {
            show_limit: 5,
            current_step: 0,
            step_move: 0,
            invalidate: true,
        }
    }
}

impl ScrollSimListWidget {
    pub fn with_show_limit(show_limit: usize) -> Self {
        Self {
            show_limit,
            ..Default::default()
        }
    }

    pub fn show_limit(&mut self, show_limit: usize) {
        self.invalidate();
        self.show_limit = show_limit;
    }

    /// Trigger reset visibilites
    pub fn invalidate(&mut self) {
        self.invalidate = true;
    }

    /// Make widget do scrolling.
    ///
    /// `step`: negative for up, otherwise down.
    pub fn scroll_to(&mut self, step: i32) {
        self.step_move = step;
    }
}

fn scroll_sim(
    mut widgets: Query<(
        Entity,
        &mut Children,
        &mut ScrollSimListWidget,
        Option<&mut ScrollProgression>,
    )>,
    mut all_widgets_query: Query<(Entity, &mut Style)>,
) {
    for (entity, children, mut widget, progression) in widgets.iter_mut() {
        if widget.step_move == 0 && !widget.invalidate {
            continue;
        }

        // check step_move
        let max_step: usize = children.len().saturating_sub(widget.show_limit);
        let target_step: usize = ((widget.current_step as i32) + widget.step_move)
            .max(0)
            .min(max_step as i32)
            .try_into()
            .unwrap();
        widget.step_move = 0;

        let actual_move: i32 = target_step as i32 - widget.current_step as i32;
        if actual_move == 0 && !widget.invalidate {
            continue;
        }

        if !widget.invalidate && actual_move.abs() < widget.show_limit as i32 {
            fix_draw(
                entity,
                &children,
                &widget,
                &mut all_widgets_query,
                actual_move,
            );
        } else {
            totally_redraw(
                entity,
                &children,
                &widget,
                &mut all_widgets_query,
                target_step,
            );
            widget.invalidate = false;
        }
        widget.current_step = target_step;

        let now: usize = target_step * 100 / max_step.max(1);

        if let Some(mut p) = progression {
            if p.0 != now {
                p.0 = now;
            }
        }
    }
}

fn fix_draw(
    entity: Entity,
    children: &Children,
    widget: &ScrollSimListWidget,
    query: &mut Query<(Entity, &mut Style)>,
    step_move: i32,
) {
    info!("fix_draw {:?} step move {}", entity, step_move);

    let ustep: usize = step_move.abs().try_into().unwrap();

    let to_drop = children.iter();
    let to_drop = if step_move > 0 {
        to_drop.skip(widget.current_step).take(ustep)
    } else {
        to_drop
            .skip((widget.current_step + widget.show_limit).saturating_sub(ustep))
            .take(ustep)
    };
    for entity in to_drop {
        match query.get_component_mut::<Style>(*entity) {
            Ok(mut style) => {
                style.display = Display::None;
                debug!("item {:?} set display none", entity);
            }
            Err(e) => debug!("item {:?} set display none error: {}", entity, e),
        }
    }

    let to_add = children.iter();
    let to_add = if step_move > 0 {
        to_add
            .skip(widget.current_step + widget.show_limit)
            .take(ustep)
    } else {
        to_add
            .skip(widget.current_step.saturating_sub(ustep))
            .take(ustep)
    };
    for entity in to_add {
        match query.get_component_mut::<Style>(*entity) {
            Ok(mut style) => {
                style.display = Display::Flex;
                debug!("item {:?} set display flex", entity);
            }
            Err(e) => debug!("item {:?} set display flex error: {}", entity, e),
        }
    }
}

fn totally_redraw(
    entity: Entity,
    children: &Children,
    widget: &ScrollSimListWidget,
    query: &mut Query<(Entity, &mut Style)>,
    target_step: usize,
) {
    info!("totally_redraw {:?} to step {}", entity, target_step);

    for (idx, entity) in children.iter().enumerate() {
        trace!("try set {} child {:?} style", idx, entity);

        match query.get_component_mut::<Style>(*entity) {
            Ok(mut style) => {
                style.display = if target_step <= idx && idx < target_step + widget.show_limit {
                    Display::Flex
                } else {
                    Display::None
                };
                debug!("{} item {:?} set display {:?}", idx, entity, style.display);
            }
            Err(e) => debug!("get_component_mut<Style> error: {}", e),
        }
    }
}
