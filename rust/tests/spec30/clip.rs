//! A clipboard app for an agent's screen: a window, so the screen gives it
//! the keyboard and with it the clipboard, that copies text and pastes what
//! the clipboard holds, as any app does. Stands for whatever the agent's
//! apps could read, or put on, the clipboard.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
    wl_data_source, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_noop, event_created_child,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use super::{TIMEOUT, wait_until};

/// The text types it copies as, and pastes, in order of preference.
const TEXT_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];

/// The types an offer has, as they come.
type Types = Arc<Mutex<Vec<String>>>;

#[derive(Default)]
struct Shared {
    /// What the clipboard holds now, as the screen last said.
    offer: Option<(wl_data_offer::WlDataOffer, Types)>,
    /// How many times the screen said the clipboard changed.
    changes: u64,
    /// The serial of the keyboard's last arrival.
    serial: Option<u32>,
}

pub struct Clip {
    connection: Connection,
    qh: QueueHandle<State>,
    manager: wl_data_device_manager::WlDataDeviceManager,
    device: wl_data_device::WlDataDevice,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    stream: UnixStream,
    thread: Option<JoinHandle<()>>,
}

struct State {
    shared: Arc<Mutex<Shared>>,
    configured: bool,
}

impl Clip {
    /// Opens the app's window on the screen at `socket`, and returns once the
    /// screen has given it the keyboard.
    pub fn start(socket: &Path) -> Clip {
        let stream = UnixStream::connect(socket)
            .unwrap_or_else(|e| panic!("can't connect to {}: {e}", socket.display()));
        let ours = stream.try_clone().unwrap();
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<State>(&connection).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let manager: wl_data_device_manager::WlDataDeviceManager = globals
            .bind(&qh, 1..=3, ())
            .unwrap_or_else(|e| panic!("the screen has no clipboard: {e}"));
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut state = State {
            shared: shared.clone(),
            configured: false,
        };
        let _keyboard = seat.get_keyboard(&qh, ());
        let device = manager.get_data_device(&seat, &qh, ());

        let surface = compositor.create_surface(&qh, ());
        let toplevel = wm.get_xdg_surface(&surface, &qh, ()).get_toplevel(&qh, ());
        toplevel.set_title("spec30 clipboard".to_owned());
        surface.commit();
        while !state.configured {
            queue.blocking_dispatch(&mut state).unwrap();
        }
        let (w, h) = (64, 64);
        let file = tempfile::tempfile().unwrap();
        file.write_all_at(&[0x40; 64 * 64 * 4], 0).unwrap();
        let pool = shm.create_pool(file.as_fd(), w * h * 4, &qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
        queue.roundtrip(&mut state).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            let keep = (
                surface, toplevel, buffer, pool, file, compositor, shm, wm, seat,
            );
            thread::spawn(move || {
                let _keep = keep;
                while !stop.load(Ordering::SeqCst) {
                    if queue.blocking_dispatch(&mut state).is_err() {
                        break;
                    }
                }
            })
        };
        let clip = Clip {
            connection,
            qh,
            manager,
            device,
            shared,
            stop,
            stream: ours,
            thread: Some(thread),
        };
        let focused = wait_until(TIMEOUT, || clip.shared.lock().unwrap().serial.is_some());
        assert!(focused, "the clipboard app never got the keyboard");
        clip
    }

    /// Copies `text`, as an app does on Ctrl+C, and waits until the screen
    /// has taken it.
    pub fn copy(&self, text: &str) {
        let before = self.shared.lock().unwrap().changes;
        let serial = self.shared.lock().unwrap().serial.unwrap_or(0);
        let source = self
            .manager
            .create_data_source(&self.qh, Arc::new(text.to_owned()));
        for t in TEXT_TYPES {
            source.offer(t.to_owned());
        }
        self.device.set_selection(Some(&source), serial);
        self.connection.flush().unwrap();
        let taken = wait_until(TIMEOUT, || self.shared.lock().unwrap().changes > before);
        assert!(taken, "the screen didn't take what the app copied");
    }

    /// What a paste in this app gets now: `None` if the clipboard is empty,
    /// or holds no text, or its owner doesn't answer.
    pub fn paste(&self) -> Option<String> {
        let (offer, types) = self.shared.lock().unwrap().offer.clone()?;
        let types = types.lock().unwrap().clone();
        let kind = TEXT_TYPES.iter().find(|t| types.iter().any(|o| o == *t))?;
        let (mut reader, writer) = std::io::pipe().unwrap();
        offer.receive((*kind).to_owned(), writer.as_fd());
        drop(writer);
        self.connection.flush().unwrap();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let mut text = String::new();
            let read = reader.read_to_string(&mut text);
            let _ = tx.send(read.map(|_| text));
        });
        rx.recv_timeout(Duration::from_secs(3)).ok()?.ok()
    }
}

impl Drop for Clip {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            state.configured = true;
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter { serial, .. } => {
                state.shared.lock().unwrap().serial = Some(serial);
            }
            wl_keyboard::Event::Key { serial, .. } => {
                state.shared.lock().unwrap().serial = Some(serial);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_device::Event::Selection { id } = event {
            let mut shared = state.shared.lock().unwrap();
            if let Some((old, _)) = shared.offer.take() {
                old.destroy();
            }
            shared.offer = id.map(|offer| {
                let types = offer.data::<Types>().cloned().unwrap_or_default();
                (offer, types)
            });
            shared.changes += 1;
        }
    }

    event_created_child!(State, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, Types::default()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, Types> for State {
    fn event(
        _: &mut Self,
        _: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        types: &Types,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            types.lock().unwrap().push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, Arc<String>> for State {
    fn event(
        _: &mut Self,
        source: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        text: &Arc<String>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { fd, .. } => {
                let _ = File::from(fd).write_all(text.as_bytes());
            }
            wl_data_source::Event::Cancelled => source.destroy(),
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ignore xdg_toplevel::XdgToplevel);
delegate_noop!(State: ignore wl_data_device_manager::WlDataDeviceManager);
