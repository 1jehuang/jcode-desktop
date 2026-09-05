#!/usr/bin/env python3
"""Private WebKit renderer. JSON-lines stdin/stdout, no app RPC or shared profile.

Linux runtime: python-gobject, python-cairo, webkit2gtk-4.1, xorg-server-xvfb.
This file is embedded in the Rust binary and launched with python3 -I.
"""
import base64
import html
import io
import json
import os
import select
import signal
import subprocess
import sys
import tempfile
import time

MAX_SOURCE = 262144
# The enclosing policy cannot be relaxed by the generated document. The iframe
# has an opaque origin, no top navigation, popups, downloads, forms or same-origin.
CSP = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; frame-src about:; connect-src 'none'; media-src 'none'; object-src 'none'; worker-src 'none'; base-uri 'none'; form-action 'none'"
CHILD_CSP = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; frame-src 'none'; connect-src 'none'; media-src 'none'; object-src 'none'; worker-src 'none'; base-uri 'none'; form-action 'none'"


def envelope(source):
    if len(source.encode('utf-8')) > MAX_SOURCE:
        raise ValueError('HTML preview exceeds 256 KiB')
    child = '<meta http-equiv="Content-Security-Policy" content="' + CHILD_CSP + '">' + source
    return ('<!doctype html><meta http-equiv="Content-Security-Policy" content="' + CSP + '">'
            '<style>html,body{margin:0;width:100%;height:100%;overflow:hidden;background:white}'
            'iframe{border:0;display:block;width:100%;height:100%}</style>'
            '<iframe sandbox="allow-scripts" referrerpolicy="no-referrer" srcdoc="' + html.escape(child, quote=True) + '"></iframe>')


def emit(value):
    sys.stdout.write(json.dumps(value, separators=(',', ':')) + '\n')
    sys.stdout.flush()


def run():
    import gi
    gi.require_version('Gtk', '3.0')
    gi.require_version('Gdk', '3.0')
    gi.require_version('WebKit2', '4.1')
    from gi.repository import Gdk, GLib, Gtk, WebKit2

    context = WebKit2.WebContext.new_ephemeral()
    context.set_sandbox_enabled(True)
    if not context.get_sandbox_enabled():
        raise RuntimeError('WebKit process sandbox is unavailable')
    context.connect('download-started', lambda _, download: download.cancel())
    view = WebKit2.WebView(web_context=context)
    settings = view.get_settings()
    settings.set_hardware_acceleration_policy(WebKit2.HardwareAccelerationPolicy.NEVER)
    settings.set_allow_file_access_from_file_urls(False)
    settings.set_allow_universal_access_from_file_urls(False)
    settings.set_javascript_can_access_clipboard(False)
    settings.set_javascript_can_open_windows_automatically(False)
    settings.set_enable_html5_database(False)
    settings.set_enable_html5_local_storage(False)
    settings.set_enable_developer_extras(False)
    settings.set_enable_media_stream(False)
    settings.set_enable_webrtc(False)
    settings.set_enable_webgl(False)
    settings.set_enable_dns_prefetching(False)
    settings.set_enable_hyperlink_auditing(False)
    view.connect('permission-request', lambda _, request: (request.deny(), True)[1])
    view.connect('context-menu', lambda *_: True)
    view.connect('create', lambda *_: None)
    view.connect('script-dialog', lambda _, dialog: True)
    # No generated document may navigate, including its own subframe. Only the
    # initial about:blank/srcdoc setup is allowed, not user-triggered navigation.
    def policy(_, decision, kind):
        if kind == WebKit2.PolicyDecisionType.NAVIGATION_ACTION:
            action = decision.get_navigation_action()
            uri = action.get_request().get_uri()
            if uri not in ('about:blank', 'about:srcdoc'):
                decision.ignore()
                return True
        elif kind == WebKit2.PolicyDecisionType.NEW_WINDOW_ACTION:
            decision.ignore()
            return True
        return False
    view.connect('decide-policy', policy)
    window = Gtk.OffscreenWindow()
    window.set_default_size(800, 420)
    window.add(view)
    window.show_all()
    view.grab_focus()
    state = {'ready': False, 'busy': False, 'dirty': True, 'ticks': 0, 'last': None, 'busy_since': 0}
    context.connect('initialize-notification-permissions', lambda *_: None)

    def snapshot():
        if state['busy'] and time.monotonic() - state['busy_since'] > 10:
            emit({'type': 'error', 'message': 'Preview stopped responding. Pause or retry the document.'})
            Gtk.main_quit()
            return False
        if not state['ready'] or state['busy'] or not state['dirty']:
            return True
        state['busy'] = True
        state['busy_since'] = time.monotonic()
        def done(widget, result):
            state['busy'] = False
            try:
                surface = widget.get_snapshot_finish(result)
                buf = io.BytesIO()
                surface.write_to_png(buf)
                image = buf.getvalue()
                if len(image) > 8 * 1024 * 1024:
                    raise ValueError('Preview image exceeds 8 MiB')
                if image != state['last']:
                    state['last'] = image
                    emit({'type': 'frame', 'png': base64.b64encode(image).decode(),
                          'width': surface.get_width(), 'height': surface.get_height()})
                state['ticks'] -= 1
                state['dirty'] = state['ticks'] > 0
            except Exception as error:
                emit({'type': 'error', 'message': str(error)})
        view.get_snapshot(WebKit2.SnapshotRegion.VISIBLE, WebKit2.SnapshotOptions.NONE, None, done)
        return True

    def dirty():
        state['dirty'] = True
        # Short bursts capture DOM changes/transitions, then idle at zero paints.
        state['ticks'] = 8

    def loaded(_, event):
        if event == WebKit2.LoadEvent.FINISHED:
            state['ready'] = True
            dirty()
    view.connect('load-changed', loaded)
    view.connect('web-process-terminated', lambda *_: (emit({'type': 'error', 'message': 'Preview browser stopped'}), Gtk.main_quit()))
    pointer_down = False

    def command(value):
        nonlocal pointer_down
        kind = value.get('type')
        if kind == 'load':
            view.load_html(envelope(value['html']), None)
        elif kind == 'resize':
            width = max(240, min(1600, int(value['width'])))
            height = max(180, min(900, int(value['height'])))
            window.set_default_size(width, height)
            window.set_size_request(width, height)
            view.set_size_request(width, height)
            window.resize(width, height)
        elif kind in ('down', 'up', 'move'):
            event_type = {'down': Gdk.EventType.BUTTON_PRESS, 'up': Gdk.EventType.BUTTON_RELEASE,
                          'move': Gdk.EventType.MOTION_NOTIFY}[kind]
            event = Gdk.Event.new(event_type)
            event.window = view.get_window()
            event.send_event = True
            event.time = Gdk.CURRENT_TIME
            event.x, event.y = float(value['x']), float(value['y'])
            event.x_root, event.y_root = event.x, event.y
            event.state = Gdk.ModifierType.BUTTON1_MASK if pointer_down else Gdk.ModifierType(0)
            if kind != 'move':
                event.button = 1
                pointer_down = kind == 'down'
            event.set_device(Gdk.Display.get_default().get_default_seat().get_pointer())
            view.event(event)
        elif kind == 'key':
            name = value.get('key', '')
            aliases = {'enter': 'Return', 'backspace': 'BackSpace', 'tab': 'Tab', 'escape': 'Escape',
                       'space': 'space', 'left': 'Left', 'right': 'Right', 'up': 'Up', 'down': 'Down',
                       'delete': 'Delete', 'home': 'Home', 'end': 'End'}
            keyval = Gdk.keyval_from_name(aliases.get(name.lower(), name))
            if not keyval and len(name) == 1:
                keyval = Gdk.unicode_to_keyval(ord(name))
            for event_type in (Gdk.EventType.KEY_PRESS, Gdk.EventType.KEY_RELEASE):
                event = Gdk.Event.new(event_type)
                event.window = view.get_window()
                event.send_event = True
                event.time = Gdk.CURRENT_TIME
                event.keyval = keyval
                event.state = Gdk.ModifierType.SHIFT_MASK if value.get('shift') else Gdk.ModifierType(0)
                event.set_device(Gdk.Display.get_default().get_default_seat().get_keyboard())
                view.event(event)
        elif kind == 'scroll':
            event = Gdk.Event.new(Gdk.EventType.SCROLL)
            event.window = view.get_window()
            event.send_event = True
            event.time = Gdk.CURRENT_TIME
            event.x, event.y = float(value['x']), float(value['y'])
            event.direction = Gdk.ScrollDirection.SMOOTH
            event.delta_x, event.delta_y = float(value.get('dx', 0)), float(value.get('dy', 0))
            event.set_device(Gdk.Display.get_default().get_default_seat().get_pointer())
            view.event(event)
        elif kind == 'refresh':
            pass
        elif kind == 'quit':
            Gtk.main_quit()
        else:
            raise ValueError('Unknown preview command')
        dirty()

    # Binary reads avoid TextIO buffering swallowing queued JSON lines.
    pending = bytearray()
    def read_commands(fd, condition):
        if condition & GLib.IO_HUP:
            Gtk.main_quit()
            return False
        chunk = os.read(fd, 65536)
        if not chunk:
            Gtk.main_quit()
            return False
        pending.extend(chunk)
        if len(pending) > MAX_SOURCE * 8:
            raise ValueError('Preview command too large')
        while b'\n' in pending:
            line, _, rest = pending.partition(b'\n')
            pending[:] = rest
            try:
                command(json.loads(line))
            except Exception as error:
                emit({'type': 'error', 'message': str(error)})
        return True
    GLib.io_add_watch(sys.stdin.fileno(), GLib.IO_IN | GLib.IO_HUP, read_commands)
    GLib.timeout_add(125, snapshot)
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, lambda: (Gtk.main_quit(), False)[1])
    emit({'type': 'ready', 'sandbox': True})
    Gtk.main()
    window.destroy()


def main():
    # Never attach GTK to the user's Wayland/X11 display or reuse their profile.
    with tempfile.TemporaryDirectory(prefix='jcode-html-') as root:
        for name in ('home', 'runtime', 'config', 'cache', 'data'):
            os.mkdir(os.path.join(root, name), 0o700)
        os.environ.update({'HOME': root + '/home', 'XDG_RUNTIME_DIR': root + '/runtime',
                           'XDG_CONFIG_HOME': root + '/config', 'XDG_CACHE_HOME': root + '/cache',
                           'XDG_DATA_HOME': root + '/data', 'GDK_BACKEND': 'x11', 'GDK_SCALE': '2',
                           'DBUS_SESSION_BUS_ADDRESS': 'unix:path=' + root + '/no-dbus',
                           'LIBGL_ALWAYS_SOFTWARE': '1'})
        read_fd, write_fd = os.pipe()
        server = subprocess.Popen(['/usr/bin/Xvfb', '-displayfd', str(write_fd), '-screen', '0',
                                   '3200x1800x24', '-nolisten', 'tcp'], pass_fds=(write_fd,),
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        os.close(write_fd)
        try:
            if not select.select([read_fd], [], [], 10)[0]:
                raise RuntimeError('Private preview display did not start')
            display = os.read(read_fd, 64).decode().strip()
            if not display.isdigit():
                raise RuntimeError('Private preview display unavailable')
            os.environ['DISPLAY'] = ':' + display
            run()
        finally:
            os.close(read_fd)
            server.terminate()
            try:
                server.wait(timeout=3)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        emit({'type': 'error', 'message': 'HTML runtime unavailable: ' + str(error)})
        sys.exit(1)
