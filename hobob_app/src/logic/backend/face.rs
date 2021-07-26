use crate::{ui::following::event::ParsedApiResult, *};
use bevy::tasks::{Task, TaskPool, TaskPoolBuilder};
use futures_lite::future;
use std::ops::Deref;

pub struct ModPlugin();

impl Plugin for ModPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.init_resource::<FaceTaskPool>()
            .add_system(show_face.system())
            .add_system(download_face.system());
    }
}

struct DownloadFace(u64, Option<std::path::PathBuf>, Option<String>);

pub struct FaceTaskPool(TaskPool);

impl FromWorld for FaceTaskPool {
    fn from_world(_world: &mut World) -> Self {
        Self(
            TaskPoolBuilder::new()
                .thread_name("face".to_string())
                .build(),
        )
    }
}

impl Deref for FaceTaskPool {
    type Target = TaskPool;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[tokio::main]
async fn do_download<T: AsRef<Path>>(url: &str, p: T) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = reqwest::get(url).await?.bytes().await?;
    debug!("downloaded {}", url);
    let t = image::io::Reader::new(std::io::Cursor::new(bytes));
    debug!("read bytes {}", url);
    let t = t.with_guessed_format()?;
    debug!("guess format {}", url);
    let t = t.decode()?;
    debug!("decoded {}", url);
    let t = t.thumbnail(256, 256);
    debug!("thumbnailed {}", url);
    t.save(p)?;
    debug!("saved {}", url);

    Ok(())
}

fn download_face(
    mut commands: Commands,
    task_pool: Res<FaceTaskPool>,
    mut result_chan: EventReader<ParsedApiResult>,
    cf: Res<AppConfig>,
) {
    for (uid, info) in result_chan
        .iter()
        .filter_map(|ParsedApiResult { uid, data }| data.as_info().map(|v| (uid, v)))
        .filter(|(_, info)| !info.face_url.is_empty())
    {
        let id = *uid;
        let url = info.face_url.clone();
        let dir = cf.face_cache_dir.clone();
        let task = task_pool.spawn(async move {
            let filename = &url[url.rfind('/').map(|x| x + 1).unwrap_or(0)..];
            let p = Path::new(&dir).join(filename);
            if !p.is_file() {
                if let Err(e) = do_download(&url, &p) {
                    error!("download {} to {:?} error: {}", url, p, e);
                    return DownloadFace(id, None, Some(e.to_string()));
                }
            }
            DownloadFace(id, Some(p), None)
        });
        commands.spawn().insert(task);
    }
}

fn show_face(
    mut commands: Commands,
    mut tasks_query: Query<(Entity, &mut Task<DownloadFace>)>,
    mut face_query: Query<(Entity, &mut Handle<ColorMaterial>, &ui::following::Face)>,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    for (entity, result) in tasks_query.iter_mut().filter_map(|(entity, mut task)| {
        future::block_on(future::poll_once(&mut *task)).map(|v| (entity, v))
    }) {
        match result.1 {
            Some(path) => {
                let uid = result.0;
                if let Some((entity, mut material, _)) =
                    face_query.iter_mut().find(|(_, _, face)| face.0 == uid)
                {
                    *material = materials.add(asset_server.load(path).into());
                    commands.entity(entity).remove::<ui::following::Face>();
                }
            }
            None => error!(
                "pull face: {}",
                result.2.expect("should return error description")
            ), // TODO alert in ui
        }
        commands.entity(entity).despawn();
    }
}
