use bevy::prelude::*;

pub struct SimpleButtonHelper();

impl Plugin for SimpleButtonHelper {
    fn build(&self, app: &mut AppBuilder) {
        app.add_system(watch_simple_display.system());
    }
}

#[derive(Clone)]
pub struct ButtonBackgroundGroup {
    pub clicked: Handle<ColorMaterial>,
    pub hovered: Handle<ColorMaterial>,
    pub none: Handle<ColorMaterial>,
}

#[allow(clippy::type_complexity)]
fn watch_simple_display(
    mut interaction_query: Query<
        (
            &Interaction,
            &ButtonBackgroundGroup,
            &mut Handle<ColorMaterial>,
        ),
        (With<Button>, Changed<Interaction>),
    >,
) {
    for (interaction, group, mut material) in interaction_query.iter_mut() {
        *material = match *interaction {
            Interaction::Clicked => group.clicked.clone(),
            Interaction::Hovered => group.hovered.clone(),
            Interaction::None => group.none.clone(),
        };
    }
}
