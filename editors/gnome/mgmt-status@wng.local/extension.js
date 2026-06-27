// mgmt status — a GNOME top-bar indicator for the mgmt pomodoro timer and next calendar event.
//
// Design: the `mgmt daemon` owns the state and pushes a *tick-stable* JSON payload over D-Bus
// whenever it changes (start/pause/skip, a phase flip, a new "next event"). A running countdown
// is encoded as an absolute `ends_at` instant, so the payload does NOT change second-to-second —
// this indicator animates the seconds itself on a local 1 Hz timer, and the daemon only has to
// push on real state changes. The click menu drives the same session by exec'ing `mgmt focus …`
// (the daemon reflects the change on its next tick).

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const OBJECT_PATH = '/org/gnome/Shell/Extensions/MgmtStatus';
const DBUS_IFACE = `
<node>
  <interface name="org.gnome.Shell.Extensions.MgmtStatus">
    <method name="Update">
      <arg type="s" name="payload" direction="in"/>
    </method>
    <method name="Clear"/>
  </interface>
</node>`;

function pad(n) {
    return String(n).padStart(2, '0');
}

function clock(secs) {
    secs = Math.max(0, Math.round(secs));
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = secs % 60;
    return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

function humanize(secs) {
    if (secs <= 0)
        return 'now';
    const mins = Math.floor(secs / 60);
    if (mins < 60)
        return `in ${mins}m`;
    if (mins < 60 * 24) {
        const h = Math.floor(mins / 60);
        const rm = mins % 60;
        return rm ? `in ${h}h${pad(rm)}m` : `in ${h}h`;
    }
    return `in ${Math.floor(mins / (60 * 24))}d`;
}

function renderPomodoro(p, now) {
    const label = p.phase === 'focus' ? 'Focus' : 'Break';
    let secs;
    if (p.running)
        secs = p.open ? now - (p.count_from ?? now) : (p.ends_at ?? now) - now;
    else
        secs = p.open ? (p.elapsed ?? 0) : (p.remaining ?? 0);
    let out = `${label} ${clock(secs)}`;
    if (!p.running)
        out += ' paused';
    return out;
}

function renderEvent(e, now) {
    if (e.all_day)
        return e.summary;
    return `${e.summary} ${humanize(e.start - now)}`;
}

const MgmtIndicator = GObject.registerClass(
class MgmtIndicator extends PanelMenu.Button {
    _init() {
        super._init(0.0, 'mgmt status');

        const box = new St.BoxLayout({style_class: 'panel-status-menu-box'});
        this._icon = new St.Icon({
            icon_name: 'alarm-symbolic',
            style_class: 'system-status-icon',
        });
        this._label = new St.Label({
            text: '',
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'mgmt-status-label',
        });
        box.add_child(this._icon);
        box.add_child(this._label);
        this.add_child(box);

        this._state = null;
        this._bin = null;
        this._tickId = 0;

        this._addItem('Start / Pause', ['focus', 'toggle']);
        this._addItem('Skip phase', ['focus', 'skip']);
        this._addItem('Stop', ['focus', 'stop']);
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this._addItem('Start pomodoro', ['focus', 'start']);
        this._addItem('Start flowtime', ['focus', 'start', '--flowtime']);

        this.hide();
    }

    _addItem(label, args) {
        const item = new PopupMenu.PopupMenuItem(label);
        item.connect('activate', () => this._exec(args));
        this.menu.addMenuItem(item);
    }

    _exec(args) {
        const bin = this._bin || 'mgmt';
        try {
            Gio.Subprocess.new([bin, ...args], Gio.SubprocessFlags.NONE).wait_async(null, null);
        } catch (e) {
            console.error(`mgmt-status: failed to run ${bin} ${args.join(' ')}: ${e}`);
        }
    }

    setPayload(payload) {
        this._state = payload;
        if (payload.bin)
            this._bin = payload.bin;
        this._render();
        this._ensureTick();
    }

    clearPayload() {
        this._state = null;
        this._stopTick();
        this._label.text = '';
        this.hide();
    }

    _ensureTick() {
        if (this._tickId)
            return;
        this._tickId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 1, () => {
            this._render();
            return GLib.SOURCE_CONTINUE;
        });
    }

    _stopTick() {
        if (this._tickId) {
            GLib.source_remove(this._tickId);
            this._tickId = 0;
        }
    }

    _render() {
        const s = this._state;
        if (!s) {
            this.hide();
            return;
        }
        const now = Math.floor(Date.now() / 1000);
        const parts = [];
        if (s.pomodoro)
            parts.push(renderPomodoro(s.pomodoro, now));
        if (s.next_event)
            parts.push(renderEvent(s.next_event, now));
        const text = parts.join('  ·  ');
        this._label.text = text;
        // A pomodoro takes icon priority; otherwise show the calendar glyph.
        this._icon.icon_name = s.pomodoro ? 'alarm-symbolic' : 'x-office-calendar-symbolic';
        if (text)
            this.show();
        else
            this.hide();
    }

    destroy() {
        this._stopTick();
        super.destroy();
    }
});

export default class MgmtStatusExtension extends Extension {
    enable() {
        this._indicator = new MgmtIndicator();
        Main.panel.addToStatusArea('mgmt-status', this._indicator);
        this._dbus = Gio.DBusExportedObject.wrapJSObject(DBUS_IFACE, this);
        this._dbus.export(Gio.DBus.session, OBJECT_PATH);
    }

    disable() {
        if (this._dbus) {
            this._dbus.unexport();
            this._dbus = null;
        }
        if (this._indicator) {
            this._indicator.destroy();
            this._indicator = null;
        }
    }

    // ── D-Bus methods (called by `mgmt daemon` via gdbus) ────────────────────
    Update(payload) {
        try {
            this._indicator?.setPayload(JSON.parse(payload));
        } catch (e) {
            console.error(`mgmt-status: bad payload: ${e}`);
        }
    }

    Clear() {
        this._indicator?.clearPayload();
    }
}
