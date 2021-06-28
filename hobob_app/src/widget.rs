use super::*;

pub fn create_following(commands: &mut Commands, app_res: &Res<AppResource>, uid: u64) -> Entity {
    let nickname = commands.spawn_bundle(TextBundle {
        text: Text::with_section(
                  format!("#{}", uid),
                  TextStyle {
                      font: app_res.font.clone(),
                      font_size: 15.,
                      color: Color::RED,
                  },
                  TextAlignment {
                      horizontal: HorizontalAlign::Center,
                      ..Default::default()
                  },
        ),
        ..Default::default()
    })
    .insert(ui::following::Nickname(uid))
    .id();

    commands.spawn_bundle(NodeBundle {
        style: Style {
            size: Size::new(Val::Percent(100.), Val::Percent(20.)),
            border: Rect::all(Val::Px(8.)),
            margin: Rect::all(Val::Px(8.)),
            ..Default::default()
        },
        material: app_res.item_bg_col.clone(),
        ..Default::default()
    }).push_children(&[nickname])
    .id()
}
