use log::error;
use tera::Tera;
use warp::{reply::Html, Filter, Rejection};

macro_rules! render {
    ($tera:ident, $name:expr, $ctx:expr) => {
        warp::reply::html($tera.render($name, $ctx).unwrap_or_else(|e| {
            let mut ctx = TeraContext::new();
            ctx.insert("kind", "Tera engine");
            ctx.insert("reason", &format!("Error: tera: {}", e));
            $tera.render("failure.html", &ctx).unwrap()
        }))
    };
}

#[derive(Clone)]
pub struct Context {
    tera: Tera,
}

macro_rules! route {
    ($t:ty, $ctx:ident) => {
        <$t>::build($ctx.clone())
    };
}

pub trait AppFilter: Filter<Extract = (Html<String>,), Error = Rejection> {}

impl<T> AppFilter for T where T: Filter<Extract = (Html<String>,), Error = Rejection> {}

mod index;

pub async fn run() {
    let tera = match Tera::new("templates/**/*.html") {
        Ok(t) => t,
        Err(e) => {
            error!("Parsing error(s): {}", e);
            return;
        }
    };

    let ctx = Context { tera };

    //let app = index::App::build(ctx.clone());
    let app = route!(index::App, ctx);
    log::info!("www running");
    warp::serve(app).run(([127, 0, 0, 1], 3000)).await;
}
