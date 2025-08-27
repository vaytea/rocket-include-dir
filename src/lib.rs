//! # Static file server, from inside the binary
//!
//! Acts as a bridge between `include_dir` and `rocket`, enabling you
//! to serve files directly out of the binary executable.
//!
//! See [`StaticFiles`] for more details.

use std::path::PathBuf;

use include_dir::File;
use rocket::fs::Options;
use rocket::http::ext::IntoOwned;
use rocket::http::uri::fmt::Path;
use rocket::http::uri::Segments;
use rocket::http::ContentType;
use rocket::http::Method;
use rocket::http::Status;
use rocket::outcome::IntoOutcome;
use rocket::response;
use rocket::response::Redirect;
use rocket::response::Responder;
use rocket::route::Handler;
use rocket::route::Outcome;
use rocket::Data;
use rocket::Request;
use rocket::Route;

pub use include_dir::include_dir;
pub use include_dir::Dir;

/// Implements a simple bridge between `include_dir` and `rocket`. A simple reponder based on
/// [`rocket::FileServer`], which uses a directory included at compile time.
///
/// ```rust
/// use rocket_include_dir::{include_dir, Dir, StaticFiles};
/// #[rocket::launch]
/// fn launch() -> _ {
///     static PROJECT_DIR: Dir = include_dir!("static");
///     build().mount("/", StaticFiles::from(&PROJECT_DIR))
/// }
/// # use rocket::{build, local::blocking::Client, http::Status};
/// # let client = Client::tracked(launch()).expect("valid rocket instance");
/// # let response = client.get("/test-doesnt-exist").dispatch();
/// # assert_eq!(response.status(), Status::NotFound);
/// # let response = client.get("/test.txt").dispatch();
/// # assert_eq!(response.status(), Status::Ok);
/// ```
#[derive(Clone, Copy)]
pub struct StaticFiles {
    dir: &'static Dir<'static>,
    options: Options,
    rank: isize,
}

impl From<&'static Dir<'static>> for StaticFiles {
    fn from(dir: &'static Dir<'static>) -> Self {
        Self {
            dir,
            options: Options::default(),
            rank: Self::DEFAULT_RANK,
        }
    }
}

impl StaticFiles {
    const DEFAULT_RANK: isize = 10;

    /// Construct a new `StaticFiles`, with the provided options.
    ///
    /// The generated route has a default rank of `10`, to match Rocket's
    /// `FileServer`
    pub fn new(dir: &'static Dir<'static>, options: Options) -> Self {
        Self {
            dir,
            options,
            rank: Self::DEFAULT_RANK,
        }
    }

    /// Replace the options for this `StaticFiles`
    pub fn options(mut self, options: Options) -> Self {
        self.options = options;
        self
    }

    /// Set a non-default rank for this `StaticFiles`
    pub fn rank(mut self, rank: isize) -> Self {
        self.rank = rank;
        self
    }
}

fn respond_with<'r>(
    req: &'r Request<'_>,
    path: PathBuf,
    file: &'r File<'r>,
) -> response::Result<'r> {
    let mut response = file.contents().respond_to(req)?;
    if let Some(ext) = path.extension() {
        if let Some(ct) = ContentType::from_extension(&ext.to_string_lossy()) {
            response.set_header(ct);
        }
    }

    Ok(response)
}

#[rocket::async_trait]
impl Handler for StaticFiles {
    async fn handle<'r>(&self, req: &'r Request<'_>, data: Data<'r>) -> Outcome<'r> {
        // TODO: Should we reject dotfiles for `self.root` if !DotFiles?
        let options = self.options;
        // Get the segments as a `PathBuf`, allowing dotfiles requested.
        let allow_dotfiles = options.contains(Options::DotFiles);
        let path = req
            .segments::<Segments<'_, Path>>(0..)
            .ok()
            .and_then(|segments| segments.to_path_buf(allow_dotfiles).ok());

        match path {
            Some(p) => {
                // If the path is empty it means the root
                let dir = if p.as_os_str().is_empty() {
                    Some(self.dir)
                } else {
                    self.dir.get_dir(&p)
                };
                if let Some(path) = dir {
                    if options.contains(Options::NormalizeDirs) && !req.uri().path().ends_with('/')
                    {
                        let normal = req
                            .uri()
                            .map_path(|p| format!("{}/", p))
                            .expect("adding a trailing slash to a known good path => valid path")
                            .into_owned();

                        return Redirect::permanent(normal)
                            .respond_to(req)
                            .or_forward((data, Status::InternalServerError));
                    }
                    if !options.contains(Options::Index) {
                        return Outcome::forward(data, Status::NotFound);
                    }
                    path.get_entry("index.html")
                        .and_then(|f| f.as_file())
                        .ok_or(Status::NotFound)
                        .and_then(|path| respond_with(req, p.join("index.html"), path))
                        .or_forward((data, Status::NotFound))
                } else if let Some(path) = self.dir.get_file(&p) {
                    respond_with(req, p, path).or_forward((data, Status::NotFound))
                } else {
                    Outcome::forward(data, Status::NotFound)
                }
            }
            None => {
                if options.contains(Options::Index) {
                    self.dir.get_entry("index.html")
                        .and_then(|f| f.as_file())
                        .ok_or(Status::NotFound)
                        .and_then(|path| respond_with(req, PathBuf::from("index.html"), path))
                        .or_forward((data, Status::NotFound))
                } else {
                    Outcome::forward(data, Status::NotFound)
                }
            }
        }
    }
}

impl From<StaticFiles> for Route {
    fn from(val: StaticFiles) -> Self {
        Route::ranked(val.rank, Method::Get, "/<path..>", val)
    }
}

impl From<StaticFiles> for Vec<Route> {
    fn from(value: StaticFiles) -> Self {
        vec![value.into()]
    }
}

#[cfg(test)]
mod tests {
    use include_dir::include_dir;
    use rocket::{build, local::blocking::Client, Build, Rocket};

    use super::*;

    fn launch() -> Rocket<Build> {
        static PROJECT_DIR: Dir = include_dir!("static");
        build()
            .mount(
                "/default",
                StaticFiles::new(&PROJECT_DIR, Options::default()),
            )
            .mount("/indexed", StaticFiles::new(&PROJECT_DIR, Options::Index))
    }

    #[test]
    fn it_works() {
        // Move current dir to avoid checking the local filesystem for path existience
        std::env::set_current_dir("/tmp").expect("Requires /tmp directory");
        let client = Client::tracked(launch()).expect("valid rocket instance");
        let response = client.get("/default/test-doesnt-exist").dispatch();
        assert_eq!(response.status(), Status::NotFound);
        let response = client.get("/default/test.txt").dispatch();
        assert_eq!(response.status(), Status::Ok);
    }

    #[test]
    fn index_file() {
        // Move current dir to avoid checking the local filesystem for path existience
        std::env::set_current_dir("/tmp").expect("Requires /tmp directory");
        let client = Client::tracked(launch()).expect("valid rocket instance");
        let response = client.get("/indexed/test-doesnt-exist").dispatch();
        assert_eq!(response.status(), Status::NotFound);
        let response = client.get("/indexed/test.txt").dispatch();
        assert_eq!(response.status(), Status::Ok);
        let response = client.get("/indexed/").dispatch();
        assert_eq!(response.status(), Status::Ok);
    }
}
