use bevy::prelude::*;

pub mod add {
    pub struct RefreshVisible();
    pub struct AddFollowing();
}

pub mod following {
    pub struct Nickname(pub u64);
    pub struct HomepageOpenButton(pub u64);
    pub struct Face(pub u64);
    pub struct LiveRoomOpenButton(pub u64);
    pub struct LiveRoomTitle(pub u64);
    pub struct VideoInfo(pub u64);

    pub mod data {
        use bevy::prelude::*;

        pub struct Info {
            uid: u64,
            nickname: String,        // Nickname
            live_room_url: String,   // LiveRoomOpenButton
            live_room_title: String, // LiveRoomTitle
            live_open: Option<bool>,
            live_entropy: u64,
            face_url: String, // request Face
        }

        pub struct Face {
            face: Handle<ColorMaterial>, // Face
        }

        pub struct NewVideo {
            date_time: String, // VideoInfo
            title: String,
        }
    }

    pub mod event {
        #[derive(Debug)]
        pub enum Action {
            RefreshVisible,
            AddFollowingUid(u64),
        }
    }
}

pub struct ResourcePlugin();

impl Plugin for ResourcePlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_event::<following::event::Action>();
    }
}
