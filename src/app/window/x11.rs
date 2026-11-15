use std::cell::Cell;
use std::ffi::c_long;
use std::rc::Rc;

use gdk4_x11::x11::xlib;
use gdk4_x11::{X11Display, X11Surface};
use gtk::prelude::*;
use gtk::{gdk, glib};

use super::Window;

pub(super) fn read_position(window: &Window) -> Option<(i32, i32)> {
    if !window.is_mapped() || window.is_maximized() || window.is_fullscreen() {
        return None;
    }
    let surface = window.surface().and_downcast::<X11Surface>()?;
    let display = surface.display().downcast::<X11Display>().ok()?;
    let xlib = xlib::Xlib::open().ok()?;
    let (mut x, mut y, mut child) = (0, 0, 0);

    // Translate from the client window even if the window manager has reparented it.
    display.error_trap_push();
    let translated = unsafe {
        (xlib.XTranslateCoordinates)(
            display.xdisplay(),
            surface.xid(),
            display.xrootwindow(),
            0,
            0,
            &mut x,
            &mut y,
            &mut child,
        )
    };
    let error = display.error_trap_pop();
    (translated != 0 && error == 0).then_some((x, y))
}

pub(super) fn setup(window: &Window, position: Option<(i32, i32)>, always_on_top: bool) {
    let Ok(display) = WidgetExt::display(window).downcast::<X11Display>() else {
        return;
    };
    WidgetExt::realize(window);
    let Some(surface) = window.surface().and_downcast::<X11Surface>() else {
        return;
    };

    let handler = Rc::new(Cell::new(None));
    let map_handler = Rc::clone(&handler);
    let xid = surface.xid();
    let surface = surface.downgrade();
    let size = window.default_size();
    // MapNotify arrives once the window manager has accepted and placed the window.
    let id = unsafe {
        display.connect_xevent(move |display, event| {
            if (*event).get_type() == xlib::MapNotify && (*event).map.window == xid {
                if let Some(id) = map_handler.take() {
                    display.disconnect(id);
                }
                if let Some(surface) = surface.upgrade() {
                    if let Some(position) = position {
                        move_to(display, &surface, position, size);
                    }
                    if always_on_top {
                        request_above(display, xid);
                    }
                }
            }
            glib::Propagation::Proceed
        })
    };
    handler.set(Some(id));
    window.connect_unrealize(move |_| {
        if let Some(id) = handler.take() {
            display.disconnect(id);
        }
    });
}

fn move_to(
    display: &X11Display,
    surface: &X11Surface,
    (x, y): (i32, i32),
    (width, height): (i32, i32),
) {
    let scale = surface.scale_factor().max(1);
    // The GDK surface size can still be stale while it dispatches MapNotify.
    let rect = gdk::Rectangle::new(x.div_euclid(scale), y.div_euclid(scale), width, height);
    let visible = display
        .monitors()
        .iter::<gdk::Monitor>()
        .flatten()
        .any(|monitor| {
            monitor
                .geometry()
                .intersect(&rect)
                .is_some_and(|area| area.width() >= 48 && area.height() >= 48)
        });
    // After a monitor is removed, let the window manager pick a new position if needed.
    if !visible {
        tracing::debug!(
            x,
            y,
            "saved window position is outside the current monitors"
        );
        return;
    }

    const MOVE_WINDOW: &str = "_NET_MOVERESIZE_WINDOW";
    #[allow(deprecated)]
    if !display.screen().supports_net_wm_hint(MOVE_WINDOW) {
        tracing::debug!("window manager does not support {MOVE_WINDOW}");
        return;
    }

    // StaticGravity takes client coordinates, so decoration offsets do not accumulate.
    let flags = xlib::StaticGravity | (1 << 8) | (1 << 9) | (1 << 12);
    send_wm_message(
        display,
        surface.xid(),
        MOVE_WINDOW,
        [flags.into(), x.into(), y.into(), 0, 0],
    );
    tracing::debug!(x, y, "requested restoration of the lyrics window position");
}

fn request_above(display: &X11Display, xid: xlib::Window) {
    const ABOVE: &str = "_NET_WM_STATE_ABOVE";
    #[allow(deprecated)]
    if !display.screen().supports_net_wm_hint(ABOVE) {
        return;
    }
    let above = gdk4_x11::x11_get_xatom_by_name_for_display(display, ABOVE);
    // Request _NET_WM_STATE_ADD with the source indication for a normal application.
    send_wm_message(display, xid, "_NET_WM_STATE", [1, above as c_long, 0, 1, 0]);
}

fn send_wm_message(display: &X11Display, xid: xlib::Window, message: &str, data: [c_long; 5]) {
    let Ok(xlib) = xlib::Xlib::open() else {
        return;
    };
    let mut event = xlib::XEvent::from(xlib::XClientMessageEvent {
        type_: xlib::ClientMessage,
        serial: 0,
        send_event: xlib::True,
        display: unsafe { display.xdisplay() },
        window: xid,
        message_type: gdk4_x11::x11_get_xatom_by_name_for_display(display, message),
        format: 32,
        data: data.into(),
    });
    unsafe {
        (xlib.XSendEvent)(
            display.xdisplay(),
            display.xrootwindow(),
            xlib::False,
            xlib::SubstructureRedirectMask | xlib::SubstructureNotifyMask,
            &mut event,
        );
        (xlib.XFlush)(display.xdisplay());
    }
}
