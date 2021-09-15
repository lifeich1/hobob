use tera::{Context as TeraContext, Tera};
use warp::Filter;

lazy_static::lazy_static! {
    pub static ref TEMPLATES: Tera = {
        let tera = match Tera::new("templates/**/*.html") {
            Ok(t) => t,
            Err(e) => {
                log::error!("Parsing error(s): {}", e);
                ::std::process::exit(1);
            }
        };
        tera
    };
}

macro_rules! render {
    ($name:expr, $ctx:expr) => {
        render!(TEMPLATES, $name, $ctx)
    };
    ($tera:ident, $name:expr, $ctx:expr) => {
        warp::reply::html($tera.render($name, $ctx).unwrap_or_else(|e| {
            let mut ctx = TeraContext::new();
            ctx.insert("kind", "Tera engine");
            ctx.insert("reason", &format!("Error: tera: {}", e));
            $tera.render("failure.html", &ctx).unwrap()
        }))
    };
}

pub async fn run() {
    let index = warp::path::end().map(|| render!("index.html", &TeraContext::new()));

    let app = index;
    log::info!("www running");
    warp::serve(app).run(([127, 0, 0, 1], 3000)).await;
}
