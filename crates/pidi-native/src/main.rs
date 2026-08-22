//! Native KAOSS/drum vertical slice. Deploy beside jambox-engine on the Pi.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use tracing::{info, warn};

use pidi_native::client::NativeClient;
use pidi_native::model::NativeModel;
use pidi_native::render::{draw, Frame};

#[derive(Parser)]
#[command(
    name = "pidi-native",
    about = "Native 800x480 KAOSS + drum slice for jambox-engine"
)]
struct Cli {
    /// Control socket path or host:port.
    #[arg(long, default_value = "/tmp/jambox.sock")]
    control: String,
    /// Connect over TCP instead of a Unix socket.
    #[arg(long)]
    tcp: bool,
    /// Presenter: dummy, fb, or auto.
    #[arg(long, default_value = "auto")]
    display: String,
    /// Linux framebuffer device.
    #[arg(long, default_value = "/dev/fb0")]
    fb: String,
    /// Linux evdev node (empty = autodetect Goodix/touch).
    #[arg(long, default_value = "")]
    evdev: String,
    /// Run N frames then exit (0 = forever).
    #[arg(long, default_value_t = 0)]
    frames: u64,
    /// Write the last frame as a PPM screenshot.
    #[arg(long)]
    dump: Option<PathBuf>,
    /// Target refresh in Hz.
    #[arg(long, default_value_t = 60)]
    hz: u32,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    let mut model = NativeModel::new();
    let mut frame = Frame::new();
    let mut client = NativeClient::new(cli.control.clone(), cli.tcp);
    let mut presenter = Presenter::open(&cli.display, &cli.fb);
    let mut input = Input::open(&cli.evdev);
    info!(
        display = presenter.name(),
        input = input.name(),
        "pidi-native: starting"
    );

    let frame_time = Duration::from_secs_f64(1.0 / cli.hz.max(1) as f64);
    let mut last = Instant::now();
    let mut status_tick = Instant::now();
    let mut n = 0u64;

    loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32();
        last = now;

        for event in input.poll() {
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
        model.connected = client.connected;
        model.status = client.last_status;
        model.tick(dt);
        draw(&mut frame, &model);
        presenter.present(&frame);

        n += 1;
        if cli.frames > 0 && n >= cli.frames {
            break;
        }
        let spent = last.elapsed();
        if spent < frame_time {
            std::thread::sleep(frame_time - spent);
        }
    }

    model.cancel_all(&mut client.outbox);
    client.flush();
    if let Some(path) = cli.dump {
        if let Err(err) = frame.write_ppm(&path) {
            warn!(%err, "native: dump failed");
        } else {
            info!(path = %path.display(), "native: wrote frame");
        }
    }
}

enum TouchEvent {
    Down { id: i32, x: i32, y: i32 },
    Move { id: i32, x: i32, y: i32 },
    Up { id: i32 },
}

enum Presenter {
    Dummy,
    #[cfg(target_os = "linux")]
    Fb(linux_fb::FbDev),
}

impl Presenter {
    fn open(kind: &str, fb: &str) -> Self {
        match kind {
            "dummy" => Self::Dummy,
            "fb" => Self::open_fb(fb).unwrap_or(Self::Dummy),
            _ => Self::open_fb(fb).unwrap_or(Self::Dummy),
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

    fn name(&self) -> &'static str {
        match self {
            Self::Dummy => "dummy",
            #[cfg(target_os = "linux")]
            Self::Fb(_) => "fbdev",
        }
    }

    fn present(&mut self, frame: &Frame) {
        match self {
            Self::Dummy => {}
            #[cfg(target_os = "linux")]
            Self::Fb(dev) => dev.blit(frame),
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

    use super::TouchEvent;

    const EV_SYN: u16 = 0;
    const EV_ABS: u16 = 3;
    const ABS_X: u16 = 0;
    const ABS_Y: u16 = 1;
    const ABS_MT_SLOT: u16 = 0x2f;
    const ABS_MT_POSITION_X: u16 = 0x35;
    const ABS_MT_POSITION_Y: u16 = 0x36;
    const ABS_MT_TRACKING_ID: u16 = 0x39;

    #[derive(Clone, Copy)]
    struct Slot {
        tracking: i32,
        x: i32,
        y: i32,
        dirty: bool,
        was_active: bool,
    }

    impl Slot {
        const fn new() -> Self {
            Self {
                tracking: -1,
                x: 0,
                y: 0,
                dirty: false,
                was_active: false,
            }
        }
    }

    pub struct Device {
        file: File,
        slots: [Slot; 10],
        current: usize,
        min_x: i32,
        max_x: i32,
        min_y: i32,
        max_y: i32,
        buf: Vec<u8>,
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
            tracing::info!(path = %path.display(), "native: evdev");
            Ok(Self {
                file,
                slots: [Slot::new(); 10],
                current: 0,
                min_x: 0,
                max_x: 800,
                min_y: 0,
                max_y: 480,
                buf: vec![0; 16 * 64],
            })
        }

        pub fn poll(&mut self) -> Vec<TouchEvent> {
            let mut out = Vec::new();
            loop {
                match self.file.read(&mut self.buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let copy = self.buf[..n].to_vec();
                        self.ingest(&copy, &mut out);
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

        fn ingest(&mut self, bytes: &[u8], out: &mut Vec<TouchEvent>) {
            let size = std::mem::size_of::<libc::input_event>();
            for chunk in bytes.chunks_exact(size) {
                let ev = unsafe { ptr_read(chunk) };
                match ev.type_ {
                    EV_ABS => match ev.code {
                        ABS_MT_SLOT => self.current = (ev.value as usize).min(self.slots.len() - 1),
                        ABS_MT_TRACKING_ID => {
                            self.slots[self.current].tracking = ev.value;
                            self.slots[self.current].dirty = true;
                        }
                        ABS_MT_POSITION_X | ABS_X => {
                            self.slots[self.current].x = ev.value;
                            self.slots[self.current].dirty = true;
                        }
                        ABS_MT_POSITION_Y | ABS_Y => {
                            self.slots[self.current].y = ev.value;
                            self.slots[self.current].dirty = true;
                        }
                        _ => {}
                    },
                    EV_SYN => {
                        if ev.code == 0 {
                            self.flush_slots(out);
                        }
                    }
                    _ => {}
                }
            }
        }

        fn flush_slots(&mut self, out: &mut Vec<TouchEvent>) {
            for (i, slot) in self.slots.iter_mut().enumerate() {
                if !slot.dirty {
                    continue;
                }
                slot.dirty = false;
                let id = i as i32;
                let x = map(slot.x, self.min_x, self.max_x, 800);
                let y = map(slot.y, self.min_y, self.max_y, 480);
                if slot.tracking < 0 {
                    if slot.was_active {
                        out.push(TouchEvent::Up { id });
                    }
                    slot.was_active = false;
                } else if !slot.was_active {
                    out.push(TouchEvent::Down { id, x, y });
                    slot.was_active = true;
                } else {
                    out.push(TouchEvent::Move { id, x, y });
                }
            }
        }
    }

    fn map(v: i32, min: i32, max: i32, screen: i32) -> i32 {
        if max <= min {
            return v.clamp(0, screen - 1);
        }
        ((v - min) as i64 * (screen as i64 - 1) / (max - min) as i64).clamp(0, screen as i64 - 1)
            as i32
    }

    fn find_touch_device() -> Option<PathBuf> {
        let dir = std::fs::read_dir("/dev/input").ok()?;
        for entry in dir.flatten() {
            let path = entry.path();
            let name = path.file_name()?.to_string_lossy();
            if !name.starts_with("event") {
                continue;
            }
            if let Ok(file) = File::open(&path) {
                let mut buf = [0u8; 256];
                let rc = unsafe {
                    libc::ioctl(file.as_raw_fd(), 0x81004506, buf.as_mut_ptr())
                };
                if rc >= 0 {
                    let label = String::from_utf8_lossy(&buf);
                    let lower = label.to_ascii_lowercase();
                    if lower.contains("goodix")
                        || lower.contains("gt911")
                        || lower.contains("touch")
                        || lower.contains("fts")
                    {
                        return Some(path);
                    }
                }
            }
        }
        None
    }

    unsafe fn ptr_read(bytes: &[u8]) -> libc::input_event {
        let mut ev = std::mem::MaybeUninit::<libc::input_event>::uninit();
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ev.as_mut_ptr() as *mut u8, bytes.len());
        ev.assume_init()
    }
}
