//! Native jambox kiosk. Deploy beside jambox-engine on the Pi.
//!
//! Default presenter is SDL + OpenGL ES 2 (KMSDRM on the Pi). fbdev is an
//! explicit fallback; dummy is for host tests and `--frames` dumps.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use tracing::{error, info, warn};

use pidi_native::client::NativeClient;
use pidi_native::input::TouchEvent;
use pidi_native::model::NativeModel;
use pidi_native::render::{rasterize, Frame};
use pidi_native::scene;

#[derive(Parser)]
#[command(
    name = "pidi-native",
    about = "Native 800x480 jambox kiosk for jambox-engine (SDL/GLES)"
)]
struct Cli {
    /// Control socket path or host:port.
    #[arg(long, default_value = "/tmp/jambox.sock")]
    control: String,
    /// Connect over TCP instead of a Unix socket.
    #[arg(long)]
    tcp: bool,
    /// Presenter: sdl, auto, dummy, or fb. auto prefers SDL, then fb, then dummy.
    #[arg(long, default_value = "auto")]
    display: String,
    /// Touch source: auto, sdl, evdev, or none.
    /// auto prefers evdev (type-A/B slots) and falls back to SDL FingerId.
    #[arg(long, default_value = "auto")]
    touch: String,
    /// Linux framebuffer device (only for --display fb).
    #[arg(long, default_value = "/dev/fb0")]
    fb: String,
    /// Linux evdev node (empty = autodetect capacitive panel).
    #[arg(long, default_value = "")]
    evdev: String,
    /// Directory of pad-01.json … pad-16.json (overrides PIDI_PHRASES_DIR).
    #[arg(long, default_value = "")]
    phrases: String,
    /// Run N frames then exit (0 = forever).
    #[arg(long, default_value_t = 0)]
    frames: u64,
    /// Write the last frame as a PPM screenshot.
    #[arg(long)]
    dump: Option<PathBuf>,
    /// Target refresh in Hz.
    #[arg(long, default_value_t = 60)]
    hz: u32,
    /// Windowed host mode (default is fullscreen for the Pi kiosk).
    #[arg(long)]
    windowed: bool,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    if !cli.phrases.is_empty() {
        std::env::set_var("PIDI_PHRASES_DIR", &cli.phrases);
    }
    let mut model = NativeModel::new();
    let mut frame = Frame::new();
    let mut client = NativeClient::new(cli.control.clone(), cli.tcp);
    let mut presenter = Presenter::open(&cli);
    // Prefer kernel type-A/B slots over SDL FingerId. SDL on KMSDRM often
    // synthesizes a single mouse from the whole pad, which is why GLOW could
    // track per-finger state and still only ever see one contact.
    let mut input = match cli.touch.as_str() {
        "none" | "sdl" => Input::None,
        _ => Input::open(&cli.evdev),
    };
    let evdev_live = input.is_evdev();
    let sdl_touch = presenter.is_sdl()
        && (cli.touch == "sdl" || (cli.touch == "auto" && !evdev_live));
    if cli.touch == "sdl" && !presenter.is_sdl() {
        warn!("native: --touch sdl ignored without an SDL presenter");
    }
    if cli.touch == "evdev" && !evdev_live {
        warn!("native: --touch evdev requested but no capacitive panel opened");
    }
    info!(
        display = presenter.name(),
        input = if sdl_touch { "sdl" } else { input.name() },
        "pidi-native: starting"
    );

    model.ensure_phrases_loaded(&mut client.outbox);
    model.ensure_library_loaded_with(Some(&mut client.outbox));
    client.flush();

    let frame_time = Duration::from_secs_f64(1.0 / cli.hz.max(1) as f64);
    let mut last = Instant::now();
    let mut status_tick = Instant::now();
    let mut n = 0u64;

    loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32();
        last = now;

        let mut events = Vec::new();
        if !presenter.poll_window(sdl_touch, &mut events) {
            break;
        }
        events.extend(input.poll());
        for event in events {
            match event {
                TouchEvent::Down { id, x, y } => model.finger_down(id, x, y, &mut client.outbox),
                TouchEvent::Move { id, x, y } => model.finger_move(id, x, y, &mut client.outbox),
                TouchEvent::Up { id } => model.finger_up(id, &mut client.outbox),
            }
        }

        if status_tick.elapsed() > Duration::from_millis(250) {
            client.outbox.status();
            status_tick = Instant::now();
        }
        client.flush();
        for notice in client.midi_inbox.drain(..) {
            model.on_midi_notice(&notice);
        }
        model.connected = client.connected;
        model.status = client.last_status;
        model.tick(dt, &mut client.outbox);
        model.maybe_autosave();
        let scene = scene::build(&model);
        presenter.present(&scene, &mut frame);

        n += 1;
        if cli.frames > 0 && n >= cli.frames {
            break;
        }
        // SDL already waits on VSync; an extra sleep makes Pi 2 hitches worse.
        if !presenter.paces_with_vsync() {
            let spent = last.elapsed();
            if spent < frame_time {
                std::thread::sleep(frame_time - spent);
            }
        }
    }

    model.cancel_all(&mut client.outbox);
    client.flush();
    if let Some(path) = cli.dump {
        rasterize(&mut frame, &scene::build(&model));
        if let Err(err) = frame.write_ppm(&path) {
            warn!(%err, "native: dump failed");
        } else {
            info!(path = %path.display(), "native: wrote frame");
        }
    }
}

enum Presenter {
    Dummy,
    #[cfg(target_os = "linux")]
    Fb(linux_fb::FbDev),
    #[cfg(feature = "sdl")]
    Sdl(pidi_native::sdl_backend::SdlDisplay),
}

impl Presenter {
    fn open(cli: &Cli) -> Self {
        let fullscreen = !cli.windowed;
        match cli.display.as_str() {
            "dummy" => Self::Dummy,
            "fb" => Self::open_fb(&cli.fb).unwrap_or_else(|| {
                warn!("native: framebuffer open failed; using dummy");
                Self::Dummy
            }),
            "sdl" => match Self::open_sdl(fullscreen) {
                Some(p) => p,
                None if cli.frames > 0 => {
                    warn!("native: SDL/GLES unavailable; dummy for --frames");
                    Self::Dummy
                }
                None => {
                    error!("native: SDL/GLES is required for --display sdl");
                    std::process::exit(1);
                }
            },
            _ => {
                if let Some(p) = Self::open_sdl(fullscreen) {
                    return p;
                }
                if cli.frames == 0 {
                    if let Some(p) = Self::open_fb(&cli.fb) {
                        warn!("native: SDL/GLES unavailable; falling back to fbdev");
                        return p;
                    }
                }
                warn!("native: SDL/GLES unavailable; using dummy");
                Self::Dummy
            }
        }
    }

    fn open_sdl(fullscreen: bool) -> Option<Self> {
        #[cfg(feature = "sdl")]
        {
            return match pidi_native::sdl_backend::SdlDisplay::open(fullscreen) {
                Ok(d) => Some(Self::Sdl(d)),
                Err(err) => {
                    warn!(%err, "native: SDL/GLES open failed");
                    None
                }
            };
        }
        #[cfg(not(feature = "sdl"))]
        {
            let _ = fullscreen;
            None
        }
    }

    fn open_fb(path: &str) -> Option<Self> {
        #[cfg(target_os = "linux")]
        {
            return linux_fb::FbDev::open(path).ok().map(Self::Fb);
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            None
        }
    }

    fn is_sdl(&self) -> bool {
        #[cfg(feature = "sdl")]
        {
            return matches!(self, Self::Sdl(_));
        }
        #[cfg(not(feature = "sdl"))]
        {
            false
        }
    }

    /// SDL present blocks on the display refresh; don't also sleep for `--hz`.
    fn paces_with_vsync(&self) -> bool {
        self.is_sdl()
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Dummy => "dummy",
            #[cfg(target_os = "linux")]
            Self::Fb(_) => "fbdev",
            #[cfg(feature = "sdl")]
            Self::Sdl(_) => "sdl-gles",
        }
    }

    fn poll_window(&mut self, collect_touch: bool, out: &mut Vec<TouchEvent>) -> bool {
        #[cfg(feature = "sdl")]
        {
            if let Self::Sdl(dev) = self {
                return dev.poll(collect_touch, out);
            }
        }
        let _ = (collect_touch, out);
        true
    }

    fn present(&mut self, scene: &scene::Scene, frame: &mut Frame) {
        match self {
            Self::Dummy => {
                rasterize(frame, scene);
            }
            #[cfg(target_os = "linux")]
            Self::Fb(dev) => {
                rasterize(frame, scene);
                dev.blit(frame);
            }
            #[cfg(feature = "sdl")]
            Self::Sdl(dev) => {
                if let Err(err) = dev.present(scene) {
                    warn!(%err, "native: SDL present failed");
                }
            }
        }
    }
}

enum Input {
    None,
    #[cfg(target_os = "linux")]
    Evdev(linux_evdev::Device),
}

impl Input {
    fn open(path: &str) -> Self {
        #[cfg(target_os = "linux")]
        {
            match linux_evdev::Device::open(path) {
                Ok(dev) => return Self::Evdev(dev),
                Err(err) => {
                    if !path.is_empty() {
                        warn!(%err, path, "native: evdev open failed");
                    }
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
        }
        Self::None
    }

    fn is_evdev(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            return matches!(self, Self::Evdev(_));
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            #[cfg(target_os = "linux")]
            Self::Evdev(_) => "evdev",
        }
    }

    fn poll(&mut self) -> Vec<TouchEvent> {
        match self {
            Self::None => Vec::new(),
            #[cfg(target_os = "linux")]
            Self::Evdev(dev) => dev.poll(),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux_fb {
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;
    use std::ptr;

    use pidi_native::render::{Frame, SCREEN_H, SCREEN_W};

    const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
    const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

    #[repr(C)]
    #[derive(Default)]
    struct FbBitfield {
        offset: u32,
        length: u32,
        msb_right: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct FbVarScreeninfo {
        xres: u32,
        yres: u32,
        xres_virtual: u32,
        yres_virtual: u32,
        xoffset: u32,
        yoffset: u32,
        bits_per_pixel: u32,
        grayscale: u32,
        red: FbBitfield,
        green: FbBitfield,
        blue: FbBitfield,
        transp: FbBitfield,
        nonstd: u32,
        activate: u32,
        height: u32,
        width: u32,
        accel_flags: u32,
        pixclock: u32,
        left_margin: u32,
        right_margin: u32,
        upper_margin: u32,
        lower_margin: u32,
        hsync_len: u32,
        vsync_len: u32,
        sync: u32,
        vmode: u32,
        rotate: u32,
        colorspace: u32,
        reserved: [u32; 4],
    }

    #[repr(C)]
    #[derive(Default)]
    struct FbFixScreeninfo {
        id: [u8; 16],
        smem_start: libc::c_ulong,
        smem_len: u32,
        type_: u32,
        type_aux: u32,
        visual: u32,
        xpanstep: u16,
        ypanstep: u16,
        ywrapstep: u16,
        line_length: u32,
        mmio_start: libc::c_ulong,
        mmio_len: u32,
        accel: u32,
        capabilities: u16,
        reserved: [u16; 2],
    }

    pub struct FbDev {
        _file: std::fs::File,
        map: *mut u8,
        map_len: usize,
        line_length: usize,
        bpp: u32,
        xres: u32,
        yres: u32,
    }

    impl FbDev {
        pub fn open(path: &str) -> std::io::Result<Self> {
            let file = OpenOptions::new().read(true).write(true).open(path)?;
            let fd = file.as_raw_fd();
            let mut vinfo = FbVarScreeninfo::default();
            let mut finfo = FbFixScreeninfo::default();
            let rc1 = unsafe { libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut vinfo) };
            let rc2 = unsafe { libc::ioctl(fd, FBIOGET_FSCREENINFO as _, &mut finfo) };
            if rc1 != 0 || rc2 != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let map_len = finfo.smem_len as usize;
            let map = unsafe {
                libc::mmap(
                    ptr::null_mut(),
                    map_len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };
            if map == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error());
            }
            tracing::info!(
                path,
                xres = vinfo.xres,
                yres = vinfo.yres,
                bpp = vinfo.bits_per_pixel,
                "native: framebuffer"
            );
            Ok(Self {
                _file: file,
                map: map as *mut u8,
                map_len,
                line_length: finfo.line_length as usize,
                bpp: vinfo.bits_per_pixel,
                xres: vinfo.xres,
                yres: vinfo.yres,
            })
        }

        pub fn blit(&mut self, frame: &Frame) {
            let w = SCREEN_W.min(self.xres as usize);
            let h = SCREEN_H.min(self.yres as usize);
            unsafe {
                match self.bpp {
                    32 | 24 => {
                        for y in 0..h {
                            let src = y * SCREEN_W;
                            let dst = self.map.add(y * self.line_length);
                            for x in 0..w {
                                let px = frame.pixels[src + x];
                                let p = dst.add(x * 4);
                                *p = px as u8;
                                *p.add(1) = (px >> 8) as u8;
                                *p.add(2) = (px >> 16) as u8;
                                if self.bpp == 32 {
                                    *p.add(3) = 0;
                                }
                            }
                        }
                    }
                    16 => {
                        for y in 0..h {
                            let src = y * SCREEN_W;
                            let dst = self.map.add(y * self.line_length) as *mut u16;
                            for x in 0..w {
                                let px = frame.pixels[src + x];
                                let r = ((px >> 16) as u16) >> 3;
                                let g = (((px >> 8) & 0xff) as u16) >> 2;
                                let b = ((px & 0xff) as u16) >> 3;
                                *dst.add(x) = (r << 11) | (g << 5) | b;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    impl Drop for FbDev {
        fn drop(&mut self) {
            unsafe {
                libc::munmap(self.map as *mut _, self.map_len);
            }
        }
    }

    unsafe impl Send for FbDev {}
}

#[cfg(target_os = "linux")]
mod linux_evdev {
    use std::fs::{File, OpenOptions};
    use std::io::Read;
    use std::os::unix::io::AsRawFd;
    use std::path::PathBuf;

    use pidi_native::evdev_touch::{
        EvdevDecoder, RawEvent, ABS_MT_POSITION_X, ABS_MT_POSITION_Y,
    };
    use pidi_native::input::TouchEvent;
    use pidi_native::render::{SCREEN_H, SCREEN_W};

    pub struct Device {
        file: File,
        decoder: EvdevDecoder,
        buf: Vec<u8>,
        pending: Vec<u8>,
    }

    impl Device {
        pub fn open(preferred: &str) -> std::io::Result<Self> {
            let path = if preferred.is_empty() {
                find_touch_device().ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no touch device")
                })?
            } else {
                PathBuf::from(preferred)
            };
            let file = OpenOptions::new().read(true).open(&path)?;
            let fd = file.as_raw_fd();
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL);
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
            let mut decoder = EvdevDecoder::new();
            decoder.set_screen(SCREEN_W as i32, SCREEN_H as i32);
            let (min_x, max_x) = read_abs_range(fd, ABS_MT_POSITION_X, (0, SCREEN_W as i32));
            let (min_y, max_y) = read_abs_range(fd, ABS_MT_POSITION_Y, (0, SCREEN_H as i32));
            decoder.set_abs_range(min_x, max_x, min_y, max_y);
            tracing::info!(
                path = %path.display(),
                min_x,
                max_x,
                min_y,
                max_y,
                "native: evdev"
            );
            Ok(Self {
                file,
                decoder,
                buf: vec![0; 16 * 64],
                pending: Vec::new(),
            })
        }

        pub fn poll(&mut self) -> Vec<TouchEvent> {
            let mut out = Vec::new();
            loop {
                match self.file.read(&mut self.buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        self.pending.extend_from_slice(&self.buf[..n]);
                        self.ingest_pending(&mut out);
                    }
                    Err(err)
                        if err.kind() == std::io::ErrorKind::WouldBlock
                            || err.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }
            out
        }

        fn ingest_pending(&mut self, out: &mut Vec<TouchEvent>) {
            let size = std::mem::size_of::<libc::input_event>();
            while self.pending.len() >= size {
                let chunk: Vec<u8> = self.pending.drain(..size).collect();
                let ev = unsafe { ptr_read(&chunk) };
                self.decoder.feed(
                    RawEvent {
                        type_: ev.type_,
                        code: ev.code,
                        value: ev.value,
                    },
                    out,
                );
            }
        }
    }

    fn read_abs_range(fd: i32, axis: u16, fallback: (i32, i32)) -> (i32, i32) {
        #[repr(C)]
        struct InputAbsinfo {
            value: i32,
            minimum: i32,
            maximum: i32,
            fuzz: i32,
            flat: i32,
            resolution: i32,
        }
        let mut info = InputAbsinfo {
            value: 0,
            minimum: 0,
            maximum: 0,
            fuzz: 0,
            flat: 0,
            resolution: 0,
        };
        // EVIOCGABS(axis) = _IOR('E', 0x40+axis, sizeof(input_absinfo)=24)
        let ioc = 0x8018_4500u32 | (0x40 + axis as u32);
        let rc = unsafe { libc::ioctl(fd, ioc as libc::c_ulong, &mut info) };
        if rc == 0 && info.maximum > info.minimum {
            (info.minimum, info.maximum)
        } else {
            fallback
        }
    }

    fn find_touch_device() -> Option<PathBuf> {
        // Prefer capacitive panels (Goodix/FT5x06) over resistive leftovers
        // like ADS7846, which also match a naive "touch" substring.
        let mut best: Option<(i32, PathBuf)> = None;
        let dir = std::fs::read_dir("/dev/input").ok()?;
        for entry in dir.flatten() {
            let path = entry.path();
            let name = path.file_name()?.to_string_lossy();
            if !name.starts_with("event") {
                continue;
            }
            let Ok(file) = File::open(&path) else {
                continue;
            };
            let mut buf = [0u8; 256];
            let rc = unsafe { libc::ioctl(file.as_raw_fd(), 0x81004506, buf.as_mut_ptr()) };
            if rc < 0 {
                continue;
            }
            let lower = String::from_utf8_lossy(&buf).to_ascii_lowercase();
            let score = if lower.contains("goodix")
                || lower.contains("gt911")
                || lower.contains("ft5")
                || lower.contains("fts")
            {
                100
            } else if lower.contains("ads7846") {
                10
            } else if lower.contains("touch") {
                50
            } else {
                continue;
            };
            if best.as_ref().map(|(s, _)| *s).unwrap_or(0) < score {
                best = Some((score, path));
            }
        }
        best.map(|(_, path)| path)
    }

    unsafe fn ptr_read(bytes: &[u8]) -> libc::input_event {
        let mut ev = std::mem::MaybeUninit::<libc::input_event>::uninit();
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ev.as_mut_ptr() as *mut u8, bytes.len());
        ev.assume_init()
    }
}
