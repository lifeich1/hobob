use bevy::{
    prelude::*,
};


pub struct ScrollWidgetsPlugin();

impl Plugin for ScrollWidgetsPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.register_type::<ScrollProgression>()
            .register_type::<ScrollSimListWidget>()
            .add_system(scroll_sim.system());
    }
}

/// Scroll progression in [0, 100]
///
/// To watch the progression, attach this component on widgets.
pub struct ScrollProgression(pub i32);

/// A scroll-simulate grid widget.
///
/// The widget will automatically control items' visibilities and push-to/pop-from children to
/// simulate scroll.
///
/// When children changed, call [`invalidate`][ScrollSimListWidget::invalidate] to notify widget.
pub struct ScrollSimListWidget{
    pub show_limit: i32,
    items: Vec<Entity>,
    current_step: i32,
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

fn scroll_sim(
    mut commands: Commands,
    widgets: Query<(Entity, &mut Children, &mut ScrollSimListWidget, Option<&mut ScrollProgression>)>,
) {
    for (entity, children, widget, progression) in widgets.iter_mut() {
        if widget.step_move == 0 && !widget.invalidate {
            continue;
        }

        // check step_move
        let max_step = (widget.items.len() - widget.show_limit).max(0);
        let target_step = (widget.current_step + widget.step_move).max(0).min(max_step);
        widget.step_move = 0;

        if !widget.invalidate && (target_step - widget.current_step).abs() < widget.show_limit {
            fix_draw(commands, entity, children, widget, target_step - widget.current_step);
        } else {
            totally_redraw(commands, entity, children, widget, target_step);
        }

        let now = target_step * 100 / max_step.max(1);

        if let Some(p) = progression {
            if p != now {
                *p = now;
            }
        }
    }
}

fn fix_draw(
    mut commands: Commands,
    entity: Entity,
    children: &Children,
    widget: &mut ScrollSimListWidget,
    step_move: i32,
) {
}

fn totally_redraw(
    mut commands: Commands,
    entity: Entity,
    children: &Children,
    widget: &mut ScrollSimListWidget,
    target_step: i32,
) {
    for child in children.iter().enumerate() {
        match idx {
            target_step..(target_step + widget.show_limit) => 
        }
    }
}
