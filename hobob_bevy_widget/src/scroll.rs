use bevy::prelude::*;
use std::convert::TryInto;

pub struct ScrollWidgetsPlugin();

impl Plugin for ScrollWidgetsPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.init_resource::<NoneColor>()
            .add_system(scroll_sim.system());
    }
}

/// Scroll progression in [0, 100]
///
/// To watch the progression, attach this component on widgets.
pub struct ScrollProgression(pub usize);

/// A scroll-simulate grid widget.
///
/// Note: currently the entity must have [`Children`][bevy::prelude::Children] component to
/// activate the system.
///
/// The widget will automatically control items' visibilities and push-to/pop-from children to
/// simulate scroll.
///
/// When children changed, call [`invalidate`][ScrollSimListWidget::invalidate] to notify widget.
pub struct ScrollSimListWidget {
    pub show_limit: usize,
    pub items: Vec<Entity>,
    current_step: usize,
    step_move: i32,
    invalidate: bool,
}

impl Default for ScrollSimListWidget {
    fn default() -> Self {
        Self {
            show_limit: 5,
            items: Vec::default(),
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

    /// Notify data changed.
    pub fn invalidate(&mut self) {
        self.invalidate = true
    }

    /// Make widget do scrolling.
    ///
    /// `step`: negative for up, otherwise down.
    pub fn scroll_to(&mut self, step: i32) {
        self.step_move = step
    }
}

struct NoneColor(Handle<ColorMaterial>);

impl FromWorld for NoneColor {
    fn from_world(world: &mut World) -> Self {
        let mut materials = world.get_resource_mut::<Assets<ColorMaterial>>().unwrap();
        Self(materials.add(Color::NONE.into()))
    }
}

fn scroll_sim(
    mut commands: Commands,
    mut widgets: Query<(
        Entity,
        &mut Children,
        &mut ScrollSimListWidget,
        Option<&mut ScrollProgression>,
    )>,
    mut all_widgets_query: Query<(Entity, &mut Style)>,
    none_col: Res<NoneColor>,
) {
    for (entity, children, mut widget, progression) in widgets.iter_mut() {
        if widget.step_move == 0 && !widget.invalidate {
            continue;
        }

        // check step_move
        let max_step: usize = widget.items.len().saturating_sub(widget.show_limit);
        let target_step: usize = ((widget.current_step as i32) + widget.step_move)
            .max(0)
            .min(max_step as i32)
            .try_into()
            .unwrap();
        widget.step_move = 0;

        let actual_move: i32 = target_step as i32 - widget.current_step as i32;
        if !widget.invalidate && actual_move.abs() < widget.show_limit as i32 {
            fix_draw(
                &mut commands,
                entity,
                &children,
                &mut widget,
                &none_col,
                &mut all_widgets_query,
                actual_move,
            );
        } else {
            totally_redraw(
                &mut commands,
                entity,
                &children,
                &mut widget,
                &none_col,
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
    commands: &mut Commands,
    entity: Entity,
    children: &Children,
    widget: &mut ScrollSimListWidget,
    none_col: &Res<NoneColor>,
    query: &mut Query<(Entity, &mut Style)>,
    step_move: i32,
) {
    info!("fix_draw {:?} step move {}", entity, step_move);

    let ustep: usize = step_move.abs().try_into().unwrap();

    let to_drop = children.iter();
    let to_drop: Vec<&Entity> = if step_move > 0 {
        to_drop.take(ustep).collect()
    } else {
        to_drop.skip(children.len().saturating_sub(ustep)).collect()
    };
    for child in to_drop {
        commands.entity(*child).despawn();
    }

    let to_drop = widget.items.iter();
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
                debug!("entity {:?} set display none", entity);
            }
            Err(e) => debug!("entity {:?} set display none error: {}", entity, e),
        }
    }

    let to_add = widget.items.iter();
    let to_add = if step_move > 0 {
        to_add
            .skip(widget.current_step + widget.show_limit)
            .take(ustep)
    } else {
        to_add
            .skip(widget.current_step.saturating_sub(ustep))
            .take(ustep)
    };
    let mut contains: Vec<Entity> = Vec::new();
    for child in to_add {
        match query.get_component_mut::<Style>(*child) {
            Ok(mut style) => {
                style.display = Display::Flex;
                debug!("entity {:?} set display flex", child);
            }
            Err(e) => debug!("entity {:?} set display flex error: {}", child, e),
        }
        let e = commands
            .spawn_bundle(contain_node_bundle(&none_col))
            .push_children(&[*child])
            .id();
        contains.push(e);
    }

    let mut e = commands.entity(entity);
    if step_move > 0 {
        e.push_children(&contains[..]);
    } else {
        e.insert_children(0, &contains[..]);
    }
}

fn totally_redraw(
    commands: &mut Commands,
    entity: Entity,
    children: &Children,
    widget: &mut ScrollSimListWidget,
    none_col: &Res<NoneColor>,
    query: &mut Query<(Entity, &mut Style)>,
    target_step: usize,
) {
    info!("totally_redraw {:?} to step {}", entity, target_step);

    for (idx, entity) in widget.items.iter().enumerate() {
        trace!("try check {} item {:?} style", idx, entity);
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

    for child in children.iter() {
        commands.entity(*child).despawn();
    }
    let mut contains: Vec<Entity> = Vec::new();
    for idx in target_step..(target_step + widget.show_limit).min(widget.items.len()) {
        let e = commands
            .spawn_bundle(contain_node_bundle(&none_col))
            .push_children(&widget.items[idx..idx + 1])
            .id();
        contains.push(e);
    }
    commands.entity(entity).push_children(&contains[..]);
}

fn contain_node_bundle(none_col: &Res<NoneColor>) -> NodeBundle {
    NodeBundle {
        style: Style {
            size: Size::new(Val::Auto, Val::Auto),
            ..Default::default()
        },
        material: none_col.0.clone(),
        ..Default::default()
    }
}
