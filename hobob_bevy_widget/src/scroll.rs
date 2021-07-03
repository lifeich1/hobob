use bevy::{ecs::query::WorldQuery, prelude::*};
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
    to_top: bool,
}

impl Default for ScrollSimListWidget {
    fn default() -> Self {
        Self {
            show_limit: 5,
            current_step: 0,
            step_move: 0,
            invalidate: true,
            to_top: false,
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

    pub fn show_limit(&mut self, show_limit: usize) -> &mut Self {
        self.invalidate();
        self.show_limit = show_limit;
        self
    }

    /// Trigger reset visibilites
    pub fn invalidate(&mut self) -> &mut Self {
        self.invalidate = true;
        self
    }

    /// Make widget do scrolling.
    ///
    /// `step`: negative for up, otherwise down.
    pub fn scroll_to(&mut self, step: i32) -> &mut Self {
        self.step_move = step;
        self
    }

    pub fn scroll_to_top(&mut self) {
        self.to_top = true;
        self.invalidate();
    }
}

fn scroll_sim(
    mut widgets: Query<(
        Entity,
        &Children,
        &mut ScrollSimListWidget,
        Option<&mut ScrollProgression>,
    )>,
    mut all_widgets_query: Query<(Option<&Children>, &mut Visible, &mut Style)>,
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

        let target_step: usize = if widget.to_top {
            widget.to_top = false;
            0_usize
        } else {
            target_step
        };

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

fn fix_draw<Q0: WorldQuery>(
    entity: Entity,
    children: &Children,
    widget: &ScrollSimListWidget,
    query: &mut Query<Q0>,
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
        subtree_set_display(*entity, query, Display::None);
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
        subtree_set_display(*entity, query, Display::Flex);
    }
}

fn totally_redraw<Q0: WorldQuery>(
    entity: Entity,
    children: &Children,
    widget: &ScrollSimListWidget,
    query: &mut Query<Q0>,
    target_step: usize,
) {
    info!("totally_redraw {:?} to step {}", entity, target_step);

    for (idx, entity) in children.iter().enumerate() {
        trace!("try set {} child {:?} style", idx, entity);

        subtree_set_display(
            *entity,
            query,
            if target_step <= idx && idx < target_step + widget.show_limit {
                Display::Flex
            } else {
                Display::None
            },
        );
    }
}

fn subtree_set_display<Q0: WorldQuery>(entity: Entity, query: &mut Query<Q0>, val: Display) {
    match query.get_component_mut::<Style>(entity) {
        Ok(mut style) => {
            style.display = val;
            trace!("entity {:?} set display {:?}", entity, val);
        }
        Err(e) => {
            trace!("entity {:?} get_component_mut<Style> error: {}", entity, e);
            return;
        }
    }
    match query.get_component_mut::<Visible>(entity) {
        Ok(mut visible) => {
            let is_visible = matches!(val, Display::Flex);
            visible.is_visible = is_visible;
            trace!("entity {:?} set is_visible {:?}", entity, is_visible);
        }
        Err(e) => {
            trace!(
                "entity {:?} get_component_mut<Visible> error: {}",
                entity,
                e
            );
        }
    }

    let mut nodes = Vec::<Entity>::new();
    if let Ok(children) = query.get_component::<Children>(entity) {
        nodes.extend(children.iter());
    }
    for child in nodes {
        subtree_set_display(child, query, val);
    }
}
