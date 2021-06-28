pub mod following {
    pub struct Nickname(pub u64);
    pub struct Face(pub u64);
    pub struct LiveRoomOpen(pub u64);
    pub struct LiveRoomTitle(pub u64);
    pub struct VideoInfo(pub u64);

    pub mod data {
        use bevy::prelude::*;

        pub struct Info {
            uid: u64,
            nickname: String, // Nickname
            live_room_url: String, // LiveRoomOpen
            live_room_title: String, // LiveRoomTitle
            live_open: bool,
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
}

