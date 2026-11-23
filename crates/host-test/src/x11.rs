//! A private X display: an Xvfb server that only this test uses, with its windows, screenshots,
//! and input.

use std::{
    path::Path,
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use eyre::{Result, WrapErr, bail, eyre};
use x11rb::{
    CURRENT_TIME,
    connection::Connection,
    protocol::{
        xproto::{
            AtomEnum, ConfigureWindowAux, ConnectionExt as _, ImageFormat, MapState, PropMode,
            Window,
        },
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

/// One viewable window.
#[derive(Debug, Clone)]
pub(crate) struct WindowInfo {
    pub(crate) id: Window,
    /// How many windows are above this window in the tree. A top-level window has 0.
    pub(crate) depth: usize,
    /// The ID of each window above this window, from the top.
    pub(crate) ancestors: Vec<Window>,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) name: String,
}

/// An Xvfb server and a connection to it. The server stops when this drops.
pub(crate) struct Display {
    number: u32,
    connection: RustConnection,
    root: Window,
    net_wm_name: u32,
    utf8_string: u32,
    /// The server comes last, so that the connection closes before the server stops.
    _server: Server,
}

/// An Xvfb process. It stops when this drops, also on an error before the display is ready.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        // A stop signal lets Xvfb remove its lock file and socket. A kill does not, and the next
        // run would then need another display number.
        let stopped = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status()
            .is_ok_and(|status| status.success());
        if stopped {
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(3) {
                if matches!(self.0.try_wait(), Ok(Some(_))) {
                    return;
                }
                sleep(Duration::from_millis(50));
            }
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Display {
    /// Starts Xvfb on a free display number, with the screen size and the `Xft.dpi` value.
    pub(crate) fn start(width: u16, height: u16, dpi: u32) -> Result<Self> {
        let number = (95..400)
            .find(|number| {
                !Path::new(&format!("/tmp/.X11-unix/X{number}")).exists()
                    && !Path::new(&format!("/tmp/.X{number}-lock")).exists()
            })
            .ok_or_else(|| eyre!("no free X display number"))?;
        let mut server = Server(
            Command::new("Xvfb")
                .arg(format!(":{number}"))
                .args(["-screen", "0", &format!("{width}x{height}x24")])
                .args(["-nolisten", "tcp", "-noreset"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .wrap_err("start Xvfb")?,
        );
        let socket = format!("/tmp/.X11-unix/X{number}");
        let started = Instant::now();
        while !Path::new(&socket).exists() {
            if let Some(status) = server.0.try_wait()? {
                bail!("Xvfb stopped: {status}");
            }
            if started.elapsed() > Duration::from_secs(10) {
                bail!("Xvfb did not start on :{number}");
            }
            sleep(Duration::from_millis(50));
        }
        let name = format!(":{number}");
        let (connection, screen) = x11rb::connect(Some(&name)).wrap_err("connect to Xvfb")?;
        let root = connection.setup().roots[screen].root;
        let net_wm_name = connection
            .intern_atom(false, b"_NET_WM_NAME")?
            .reply()?
            .atom;
        let utf8_string = connection.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
        // baseview and REAPER read the display scale from Xft.dpi in the resource database.
        connection.change_property8(
            PropMode::REPLACE,
            root,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            format!("Xft.dpi:\t{dpi}\n").as_bytes(),
        )?;
        connection.flush()?;
        Ok(Self {
            number,
            _server: server,
            connection,
            root,
            net_wm_name,
            utf8_string,
        })
    }

    /// The `Xft.dpi` value that a new client of the display reads.
    pub(crate) fn xft_dpi(&self) -> Result<Option<u32>> {
        let database = x11rb::resource_manager::new_from_default(&self.connection)?;
        Ok(database.get_value::<u32>("Xft.dpi", "")?)
    }

    /// The value for `DISPLAY`.
    pub(crate) fn name(&self) -> String {
        format!(":{}", self.number)
    }

    /// Each viewable window, in tree order.
    pub(crate) fn windows(&self) -> Result<Vec<WindowInfo>> {
        let mut found = Vec::new();
        let mut path = Vec::new();
        self.walk(self.root, &mut path, &mut found)?;
        Ok(found)
    }

    fn walk(
        &self,
        window: Window,
        path: &mut Vec<Window>,
        found: &mut Vec<WindowInfo>,
    ) -> Result<()> {
        let Ok(tree) = self.connection.query_tree(window)?.reply() else {
            return Ok(());
        };
        for child in tree.children {
            // A window can close during the walk.
            let Ok(attributes) = self.connection.get_window_attributes(child)?.reply() else {
                continue;
            };
            if attributes.map_state != MapState::VIEWABLE {
                continue;
            }
            let (Ok(geometry), Ok(origin)) = (
                self.connection.get_geometry(child)?.reply(),
                self.connection
                    .translate_coordinates(child, self.root, 0, 0)?
                    .reply(),
            ) else {
                continue;
            };
            found.push(WindowInfo {
                id: child,
                depth: path.len(),
                ancestors: path.clone(),
                x: i32::from(origin.dst_x),
                y: i32::from(origin.dst_y),
                width: u32::from(geometry.width),
                height: u32::from(geometry.height),
                name: self.window_name(child)?,
            });
            path.push(child);
            self.walk(child, path, found)?;
            path.pop();
        }
        Ok(())
    }

    fn window_name(&self, window: Window) -> Result<String> {
        for (property, kind) in [
            (self.net_wm_name, self.utf8_string),
            (AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()),
        ] {
            if let Ok(reply) = self
                .connection
                .get_property(false, window, property, kind, 0, 256)?
                .reply()
                && !reply.value.is_empty()
            {
                return Ok(String::from_utf8_lossy(&reply.value).into_owned());
            }
        }
        Ok(String::new())
    }

    /// Writes the screen as a PNG file.
    pub(crate) fn screenshot(&self, path: &Path) -> Result<()> {
        let geometry = self.connection.get_geometry(self.root)?.reply()?;
        let (width, height) = (geometry.width, geometry.height);
        let image = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                0,
                0,
                width,
                height,
                u32::MAX,
            )?
            .reply()?;
        let pixels = usize::from(width) * usize::from(height);
        if image.data.len() < pixels * 4 {
            bail!("the screen image has {} bytes", image.data.len());
        }
        // A 24-bit screen holds each pixel in 4 bytes: blue, green, red, and one unused byte.
        let rgb = image.data[..pixels * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]])
            .collect();
        let picture = image::RgbImage::from_raw(u32::from(width), u32::from(height), rgb)
            .ok_or_else(|| eyre!("the screen image has the wrong size"))?;
        picture
            .save(path)
            .wrap_err_with(|| format!("write {}", path.display()))
    }

    /// Asks for a new size of a window, as a window manager does.
    pub(crate) fn resize(&self, window: Window, width: u32, height: u32) -> Result<()> {
        self.connection.configure_window(
            window,
            &ConfigureWindowAux::new().width(width).height(height),
        )?;
        self.connection.flush()?;
        Ok(())
    }

    /// Moves the pointer to a screen position and clicks the left button.
    pub(crate) fn click(&self, x: i32, y: i32) -> Result<()> {
        let x = i16::try_from(x).wrap_err("x")?;
        let y = i16::try_from(y).wrap_err("y")?;
        let motion = x11rb::protocol::xproto::MOTION_NOTIFY_EVENT;
        let press = x11rb::protocol::xproto::BUTTON_PRESS_EVENT;
        let release = x11rb::protocol::xproto::BUTTON_RELEASE_EVENT;
        for (kind, detail, pause) in [(motion, 0, 150), (press, 1, 60), (release, 1, 100)] {
            self.connection
                .xtest_fake_input(kind, detail, CURRENT_TIME, self.root, x, y, 0)?;
            self.connection.flush()?;
            sleep(Duration::from_millis(pause));
        }
        Ok(())
    }
}
