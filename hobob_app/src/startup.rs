use bevy::prelude::*;
use super::{AppConfig, AppResource};
use hobob_bevy_widget::scroll;

pub fn ui(
    mut commands: Commands,
    app_res: Res<AppResource>,
    cf: Res<AppConfig>,
) {
    let root = commands.spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(100.0), Val::Percent(100.0)),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                ..Default::default()
            },
            material: app_res.bg_col.clone(),
            ..Default::default()
        }).id();

    if let Some(e) = cf.startup_error.as_ref() {
        commands.entity(root).with_children(|parent| {
            parent.spawn_bundle(TextBundle {
                text: Text::with_section(
                          format!("STARTUP ERROR: {}", e),
                          TextStyle {
                              font: app_res.font.clone(),
                              font_size: 30.,
                              color: app_res.err_text_col,
                          },
                          TextAlignment {
                              horizontal: HorizontalAlign::Center,
                              ..Default::default()
                          },
                      ),
                ..Default::default()
            });
        });
        return;
    }

    commands.entity(root).with_children(|parent| {
        // node for uid input widget
        parent.spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Auto, Val::Px(100.)),
                ..Default::default()
            },
            ..Default::default()
        });

        // followings browser
        parent.spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(80.0), Val::Percent(80.0)),
                flex_direction: FlexDirection::ColumnReverse,
                flex_grow: 100.0,
                ..Default::default()
            },
            ..Default::default()
        })
        .insert(scroll::ScrollSimListWidget::default());
    });
}
