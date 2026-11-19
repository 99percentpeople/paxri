"""Exercise reordering, tear-off and docking in both tab layouts on a Windows desktop with pywin32.

Run `py scripts/check-tab-drag-windows.py` from the project checkout.
The check uses a separate GUI, moves the desktop cursor, then restores it.
"""
import argparse
import ctypes
import subprocess
import sys
import time
import uuid
from contextlib import contextmanager
from ctypes import wintypes
from pathlib import Path


def check(binary, sidebar_only=False):
    import win32api as api
    import win32con as win
    import win32gui as gui
    import win32process as process
    import win32ui

    user32 = ctypes.windll.user32
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    swapped = api.GetSystemMetrics(win.SM_SWAPBUTTON)
    primary = win.VK_RBUTTON if swapped else win.VK_LBUTTON
    down = win.MOUSEEVENTF_RIGHTDOWN if swapped else win.MOUSEEVENTF_LEFTDOWN
    up = win.MOUSEEVENTF_RIGHTUP if swapped else win.MOUSEEVENTF_LEFTUP
    if api.GetAsyncKeyState(primary) & 0x8000:
        raise RuntimeError("The primary mouse button is in use")

    app = subprocess.Popen([str(binary), "--ipc", "tab-test-" + uuid.uuid4().hex],
                           creationflags=subprocess.CREATE_NO_WINDOW)
    original = api.GetCursorPos()
    pressed = False

    def window(title):
        matches = []

        def collect(hwnd, _):
            if (process.GetWindowThreadProcessId(hwnd)[1] == app.pid
                    and gui.IsWindowVisible(hwnd) and gui.GetWindowText(hwnd) == title):
                matches.append(hwnd)

        gui.EnumWindows(collect, None)
        return next(iter(matches), None)

    def wait_window(title, seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end and app.poll() is None:
            hwnd = window(title)
            if hwnd:
                return hwnd
            time.sleep(0.03)
        raise RuntimeError(f"Window did not appear: {title}")

    def key(hwnd, code):
        scan = api.MapVirtualKey(code, 0) << 16
        gui.PostMessage(hwnd, win.WM_KEYDOWN, code, scan | 1)
        gui.PostMessage(hwnd, win.WM_KEYUP, code, scan | 1 | (3 << 30))
        time.sleep(0.15)

    def screen_color(position):
        dc = gui.GetDC(0)
        try:
            return ctypes.windll.gdi32.GetPixel(ctypes.c_void_p(dc), *position)
        finally:
            gui.ReleaseDC(0, dc)

    @contextmanager
    def screen_region(origin, size):
        screen = win32ui.CreateDCFromHandle(gui.CreateDC("DISPLAY", None, None))
        memory = screen.CreateCompatibleDC()
        bitmap = win32ui.CreateBitmap()
        bitmap.CreateCompatibleBitmap(screen, *size)
        previous = memory.SelectObject(bitmap)

        def read():
            memory.BitBlt((0, 0), size, screen, origin, win.SRCCOPY)
            return bitmap.GetBitmapBits(True)

        try:
            yield read
        finally:
            memory.SelectObject(previous)
            gui.DeleteObject(bitmap.GetHandle())
            memory.DeleteDC()
            screen.DeleteDC()

    def page_heading(hwnd):
        # Read the page title below the tab bar, independent of hover and drag styles.
        scale = user32.GetDpiForWindow(hwnd) / 96
        origin = gui.ClientToScreen(hwnd, (round(14 * scale), round(52 * scale)))
        with screen_region(origin, (round(145 * scale), round(40 * scale))) as read:
            return read()

    class GuiThreadInfo(ctypes.Structure):
        _fields_ = [
            ("cbSize", wintypes.DWORD), ("flags", wintypes.DWORD),
            ("hwndActive", wintypes.HWND), ("hwndFocus", wintypes.HWND),
            ("hwndCapture", wintypes.HWND), ("hwndMenuOwner", wintypes.HWND),
            ("hwndMoveSize", wintypes.HWND), ("hwndCaret", wintypes.HWND),
            ("rcCaret", wintypes.RECT),
        ]

    def check_held_window(hwnd, heading=None):
        info = GuiThreadInfo()
        info.cbSize = ctypes.sizeof(info)
        end = time.monotonic() + 2
        while True:
            if not api.GetAsyncKeyState(primary) & 0x8000:
                raise AssertionError("Mouse released before checking the dragged tab")
            focused = (user32.GetGUIThreadInfo(0, ctypes.byref(info))
                       and gui.GetForegroundWindow() == hwnd == info.hwndActive
                       and (info.hwndFocus == hwnd
                            or (info.hwndFocus and gui.IsChild(hwnd, info.hwndFocus))))
            if focused and (heading is None or page_heading(hwnd) == heading):
                return
            if time.monotonic() >= end:
                if not focused:
                    raise AssertionError("The window owning the dragged tab must have keyboard focus")
                raise AssertionError("The active window must display the dragged page before release")
            time.sleep(0.015)

    try:
        home = wait_window("P2P Tunnel", 12)
        left, top, right, bottom = api.GetMonitorInfo(api.MonitorFromWindow(home))["Work"]
        width, height = right - left, bottom - top
        dpi = user32.GetDpiForWindow(home) / 96
        frame_width = min(width - 24, max(width // 2, round(1080 * dpi)))
        gui.SetWindowPos(home, win.HWND_TOPMOST, left + 12, top + height // 8,
                         frame_width, height * 2 // 3, 0)
        gui.SetWindowPos(home, win.HWND_NOTOPMOST, 0, 0, 0, 0, win.SWP_NOMOVE | win.SWP_NOSIZE)
        time.sleep(0.3)
        home_heading = page_heading(home)
        source_size = gui.GetClientRect(home)[2:]
        key(home, win.VK_DOWN)   # Menu has initial keyboard focus.
        key(home, win.VK_RETURN)  # Settings
        dpi = user32.GetDpiForWindow(home) / 96

        def home_point(x, y):
            return gui.ClientToScreen(home, (round(x * dpi), round(y * dpi)))

        if sidebar_only:
            key(home, win.VK_TAB)  # From the selected tab to Menu.
            key(home, win.VK_UP)
            key(home, win.VK_RETURN)  # Tabs on left.
            probe_y = round(gui.GetClientRect(home)[3] / dpi) - 22

            def expect_width(width):
                # The divider is a one-pixel border even if the page and rail share a color.
                edge, y = home_point(width, probe_y)
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    sides = {screen_color(home_point(width + dx, probe_y)) for dx in (-12, 12)}
                    if any(screen_color((x, y)) not in sides
                           for x in range(edge - round(2 * dpi), edge + 1)):
                        return
                    time.sleep(0.03)
                raise AssertionError(f"Expected the sidebar divider at {width} logical pixels")

            def check_compact_tab():
                # Settings is the selected second tab. Its face is visible against the rail.
                left, top = home_point(2, 77)
                right, bottom = home_point(40, 115)
                width, height = right - left, bottom - top
                with screen_region((left, top), (width, height)) as read:
                    pixels = read()
                rail = pixels[:3]
                x_edges, y_edges = [], []
                for y in range(height):
                    for x in range(width):
                        offset = (y * width + x) * 4
                        if pixels[offset:offset + 3] != rail:
                            x_edges.append(x)
                            y_edges.append(y)
                if not x_edges:
                    raise AssertionError("The selected compact tab is missing")
                face_width = max(x_edges) - min(x_edges) + 1
                face_height = max(y_edges) - min(y_edges) + 1
                center = left + (min(x_edges) + max(x_edges) + 1) / 2
                if (abs(face_width - 32 * dpi) > 1 or abs(face_height - 32 * dpi) > 1
                        or abs(center - home_point(22, 96)[0]) > 1):
                    raise AssertionError(f"Expected a centered 32px square tab, got {face_width}x{face_height} device pixels")

            def toggle_sidebar():
                api.SetCursorPos(home_point(20, 20))
                api.mouse_event(down, 0, 0)
                api.mouse_event(up, 0, 0)
                time.sleep(0.12)

            def resize_sidebar(old, new):
                nonlocal pressed
                api.SetCursorPos(home_point(old - 3, probe_y - 35))
                api.mouse_event(down, 0, 0)
                pressed = True
                api.SetCursorPos(home_point(new - 3, probe_y - 35))
                expect_width(new)
                api.mouse_event(up, 0, 0)
                pressed = False

            expect_width(142)
            resize_sidebar(142, 284)
            toggle_sidebar()
            expect_width(44)
            check_compact_tab()
            toggle_sidebar()
            expect_width(284)
            toggle_sidebar()
            expect_width(44)
            resize_sidebar(44, 192)
            api.SetCursorPos(home_point(350, probe_y))
            expect_width(192)
            print("Passed. The compact tabs are centered squares and the sidebar resized and restored its width.")
            return

        trailing_x = max(180, min(350, round(gui.GetClientRect(home)[2] / dpi) - 250))
        # Sample the selected face with the pointer off the tabs so its hover
        # color doesn't become the reference for the floating preview.
        api.SetCursorPos(home_point(149, 75))
        time.sleep(0.1)
        tab_fill = screen_color(home_point(149, 10))
        api.SetCursorPos(home_point(149, 20))
        time.sleep(0.1)
        tab_hover_fill = screen_color(home_point(149, 10))
        api.SetCursorPos(home_point(149, 75))
        time.sleep(0.1)

        def check_trailing_preview():
            if trailing_x > 250:
                end = time.monotonic() + 2
                while screen_color(home_point(trailing_x, 10)) != tab_fill:
                    if time.monotonic() >= end:
                        raise AssertionError("The held tab did not reach the blank titlebar space")
                    time.sleep(0.025)

        def dock_in_trailing_space(detached, drop):
            if trailing_x <= 250:
                api.SetCursorPos(drop)
                return
            # Capture one scanline of the composed desktop. The resting Settings
            # slot stays empty while the incoming tab is held farther to its right.
            home_x, slot_x, held_x = (round(x * dpi) for x in (40, 149, trailing_x))

            def color(row, x):
                return row[4 * x:4 * x + 3]

            with screen_region(home_point(0, 10), (held_x + 1, 1)) as read:
                row = read()
                empty = color(row, slot_x)
                # GetPixel's COLORREF in BGR byte order matches the scanline.
                # The Home tab may still have its hover color after keyboard navigation.
                selected = tab_fill.to_bytes(3, "big")
                if empty == color(row, home_x) or color(row, held_x) != empty:
                    raise AssertionError("Expected a single Home tab before docking")
                api.SetCursorPos(drop)
                start = time.monotonic()
                while time.monotonic() - start < 2:
                    row = read()
                    if color(row, slot_x) != empty:
                        raise AssertionError("The held tab flashed in its resting slot while docking")
                    if (time.monotonic() - start >= 0.2 and not gui.IsWindow(detached)
                            and color(row, held_x) == selected and color(row, home_x) != selected):
                        return
                raise AssertionError("The receiving strip did not paint the held tab at the pointer")

        if trailing_x > 250 and screen_color(home_point(trailing_x, 10)) == tab_fill:
            raise AssertionError("Expected visible blank space beyond the resting tabs")
        settings_heading = page_heading(home)

        def check_detached_frames(hwnd):
            # Window geometry can lead the compositor by a refresh during a move.
            # Once it catches up, every displayed frame should contain the page.
            ctypes.windll.dwmapi.DwmFlush()
            for _ in range(4):
                ctypes.windll.dwmapi.DwmFlush()
                if page_heading(hwnd) != settings_heading:
                    raise AssertionError("The detached Settings window showed an incomplete frame")

        def check_floating_slot(hwnd, sidebar, second):
            scale = user32.GetDpiForWindow(hwnd) / 96
            probes = ((13, 52), (13, 87)) if sidebar else ((20, 10), (180, 10))
            end = time.monotonic() + 1
            while time.monotonic() < end:
                selected = []
                for x, y in probes:
                    point = gui.ClientToScreen(hwnd, (round(x * scale), round(y * scale)))
                    color = screen_color(point)
                    selected.append(color in (tab_fill, tab_hover_fill))
                if selected == [not second, second]:
                    return
                ctypes.windll.dwmapi.DwmFlush()
            raise AssertionError(f"Wrong floating tab slot (sidebar={sidebar}, second={second})")

        def click_home_tab(y=20):
            nonlocal pressed
            pos = home_point(45, y)
            if gui.GetAncestor(gui.WindowFromPoint(pos), win.GA_ROOT) != home:
                raise RuntimeError("The Home tab is covered by another window")
            api.SetCursorPos(pos)
            api.mouse_event(down, 0, 0)
            pressed = True
            api.mouse_event(up, 0, 0)
            pressed = False
            time.sleep(0.2)

        def check_close_position(row_y, strip_right, home_y):
            # Settings is last. Its close glyph is the rightmost ink in this row.
            left, top = home_point(4, row_y - 9)
            right, bottom = home_point(strip_right, row_y + 9)
            width = right - left

            def glyph_edge(pixels):
                for x in reversed(range(width)):
                    # Compare against the background at the top of this column.
                    # The close glyph can be lighter or darker than its tab.
                    background = pixels[x * 4:x * 4 + 3]
                    if any(max(abs(a - b) for a, b in zip(pixels[i:i + 3], background)) > 60
                           for i in range(x * 4, len(pixels), width * 4)):
                        return x
                raise AssertionError("The Settings close glyph is missing")

            with screen_region((left, top), (width, bottom - top)) as read:
                selected = glyph_edge(read())
                click_home_tab(home_y)
                api.SetCursorPos(home_point(300, 150))
                inactive = glyph_edge(read())
            # Text antialiasing can change a fringe pixel with the foreground color.
            if abs(selected - inactive) > 1:
                raise AssertionError(f"Selecting Settings shifted the close glyph by {selected - inactive}px")

        def choose_existing_settings(owner):
            key(home, win.VK_TAB)  # From Home to Menu
            key(home, win.VK_DOWN)
            key(home, win.VK_RETURN)
            end = time.monotonic() + 2
            while time.monotonic() < end:
                if (gui.GetForegroundWindow() == owner and not gui.IsIconic(owner)
                        and page_heading(owner) == settings_heading):
                    return
                time.sleep(0.025)
            raise AssertionError("Menu > Settings did not activate the existing Settings page")

        check_close_position(20, 260, 20)
        if page_heading(home) == settings_heading:
            raise AssertionError("Expected Home to be visible before selecting Settings")
        choose_existing_settings(home)
        click_home_tab()
        start = home_point(149, 20)
        api.SetCursorPos(start)
        time.sleep(0.15)
        if gui.GetAncestor(gui.WindowFromPoint(start), win.GA_ROOT) != home:
            raise RuntimeError("The test tab is covered by another window")
        api.mouse_event(down, 0, 0)
        pressed = True
        check_held_window(home, settings_heading)
        # The strip is 40 logical pixels tall. Stay within its 24 pixel
        # clearance while passing the last tab, then cross it by two pixels.
        for x, y in [(26, 58), (trailing_x, 58), (149, 62)]:
            api.SetCursorPos(home_point(x, y))
            time.sleep(0.18)
            if window("Settings — P2P Tunnel"):
                raise AssertionError("Split before the tab left the reorder threshold")
            if x == trailing_x:
                check_trailing_preview()
        api.SetCursorPos(home_point(149, 66))
        detached = wait_window("Settings — P2P Tunnel", 4)
        check_held_window(detached)
        check_detached_frames(detached)
        if gui.GetClientRect(detached)[2:] != source_size:
            raise AssertionError("The detached window did not inherit the source client size")
        check_floating_slot(detached, sidebar=False, second=True)

        points = [(left + width * 2 // 3, top + height // 3),
                  (left + width * 4 // 5, top + height // 2)]
        for pt in points:
            api.SetCursorPos(pt)
            end = time.monotonic() + 2
            while True:
                x, y = gui.ScreenToClient(detached, pt)
                visible = gui.GetAncestor(gui.WindowFromPoint(pt), win.GA_ROOT) == detached
                if visible and abs(x - 149 * dpi) <= 3 and abs(y - 20 * dpi) <= 3:
                    break
                if time.monotonic() > end:
                    raise AssertionError(f"Floating tab missed cursor {pt}, local offset {(x, y)}")
                time.sleep(0.02)
            if not api.GetAsyncKeyState(primary) & 0x8000:
                raise AssertionError("Mouse released before the window followed")
        if page_heading(home) != home_heading:
            raise AssertionError("The source window must show Home while the detached tab is held")
        api.mouse_event(up, 0, 0)
        pressed = False
        check_detached_frames(detached)
        check_floating_slot(detached, sidebar=False, second=False)
        box = gui.GetWindowRect(detached)
        api.SetCursorPos(start)
        time.sleep(0.15)
        if window("Settings — P2P Tunnel") != detached or gui.GetWindowRect(detached) != box:
            raise AssertionError("Detached window changed after release")
        gui.ShowWindow(detached, win.SW_MINIMIZE)
        click_home_tab()
        choose_existing_settings(detached)
        # Release, then start an independent drag on the floating tab. A drop
        # into the page body should leave both windows available.
        for y, should_dock in [(120, False), (20, True)]:
            grab = gui.ClientToScreen(detached, (round(45 * dpi), round(20 * dpi)))
            api.SetCursorPos(grab)
            time.sleep(0.12)
            if gui.GetAncestor(gui.WindowFromPoint(grab), win.GA_ROOT) != detached:
                raise RuntimeError("The floating test tab is covered by another window")
            api.mouse_event(down, 0, 0)
            pressed = True
            time.sleep(0.08)
            before = gui.ClientToScreen(detached, (0, 0))
            api.SetCursorPos((grab[0] + round(18 * dpi), grab[1]))
            time.sleep(0.22)
            moved = gui.ClientToScreen(detached, (0, 0))
            if abs(moved[0] - before[0] - round(18 * dpi)) > 3 or abs(moved[1] - before[1]) > 3:
                raise AssertionError("Single-tab window did not follow inside the tab row")
            api.SetCursorPos((grab[0], grab[1] + round(85 * dpi)))
            time.sleep(0.2)
            drop = home_point(trailing_x if should_dock else 180, y)
            if should_dock:
                dock_in_trailing_space(detached, drop)
                end = time.monotonic() + 2
                while gui.IsWindow(detached) and time.monotonic() < end:
                    time.sleep(0.025)
                if gui.IsWindow(detached) or not api.GetAsyncKeyState(primary) & 0x8000:
                    raise AssertionError("The receiver must own both tabs before mouse-up")
                check_held_window(home, settings_heading)
                check_trailing_preview()
                # Keep the original press. Reorder to the first slot, tear it out
                # again, then re-enter the receiver and finish the same gesture.
                api.SetCursorPos(home_point(16, 20))
                time.sleep(0.22)
                api.SetCursorPos(home_point(16, 92))
                detached = wait_window("Settings — P2P Tunnel", 2)
                api.SetCursorPos(home_point(160, 20))
                end = time.monotonic() + 2
                while gui.IsWindow(detached) and time.monotonic() < end:
                    time.sleep(0.025)
                if gui.IsWindow(detached) or not api.GetAsyncKeyState(primary) & 0x8000:
                    raise AssertionError("Second entry did not merge during the same press")
            else:
                api.SetCursorPos(drop)
                time.sleep(0.22)
                if not gui.IsWindow(detached):
                    raise AssertionError("Docked over the page body")
            api.mouse_event(up, 0, 0)
            pressed = False
            time.sleep(0.13)
            if bool(gui.IsWindow(detached)) == should_dock or window("P2P Tunnel") != home:
                raise AssertionError(f"Unexpected docking result for destination {drop}")
        key(home, win.VK_TAB)     # From the selected tab to Menu.
        key(home, win.VK_UP)      # Open Menu on its last item.
        key(home, win.VK_RETURN)  # Tabs on left
        client_height = round(gui.GetClientRect(home)[3] / dpi)
        middle_y, bottom_y = client_height // 2, client_height - 8

        def check_sidebar(y, selected=True):
            end = time.monotonic() + 2
            while (screen_color(home_point(14, y)) == tab_fill) != selected:
                if time.monotonic() >= end:
                    expected = "the selected tab" if selected else "empty space"
                    raise AssertionError(f"Expected {expected} at sidebar y={y}")
                time.sleep(0.015)

        check_sidebar(94)  # Settings is selected in the second slot.
        check_sidebar(bottom_y, False)
        api.SetCursorPos(home_point(45, 60))  # Home, first slot
        api.mouse_event(down, 0, 0)
        pressed = True
        api.SetCursorPos(home_point(45, 70))
        for y in (middle_y, bottom_y):
            api.SetCursorPos(home_point(45, y))
            check_sidebar(y)
        api.mouse_event(up, 0, 0)
        pressed = False
        check_sidebar(94)
        check_sidebar(bottom_y, False)

        # Tear Settings out of the first slot and enter the bottom of the sidebar.
        api.SetCursorPos(home_point(45, 60))
        api.mouse_event(down, 0, 0)
        pressed = True
        api.SetCursorPos(home_point(195, 60))
        detached = wait_window("Settings — P2P Tunnel", 4)
        check_held_window(detached)
        api.SetCursorPos(home_point(45, bottom_y))
        check_held_window(home)
        check_sidebar(bottom_y)
        api.SetCursorPos(home_point(45, middle_y))
        check_sidebar(middle_y)
        api.mouse_event(up, 0, 0)
        pressed = False
        check_sidebar(94)
        check_sidebar(middle_y, False)
        if gui.IsWindow(detached):
            raise AssertionError("The floating Settings window did not dock in the sidebar")
        check_close_position(94, 138, 60)
        check_sidebar(60)
        # Tear out the second sidebar row. Keep its row until the original
        # press ends, including when the pointer is stationary on release.
        api.SetCursorPos(home_point(45, 94))
        api.mouse_event(down, 0, 0)
        pressed = True
        api.SetCursorPos(home_point(195, 94))
        detached = wait_window("Settings — P2P Tunnel", 4)
        api.SetCursorPos(points[0])
        check_held_window(detached)
        check_floating_slot(detached, sidebar=True, second=True)
        api.mouse_event(up, 0, 0)
        pressed = False
        check_floating_slot(detached, sidebar=True, second=False)
        print("Passed. Tabs tracked the pointer through reordering, tear-off, and docking in both strips.")
    finally:
        if pressed:
            api.mouse_event(up, 0, 0)
        api.SetCursorPos(original)
        if app.poll() is None:
            app.terminate()
        app.wait(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/tunnel-gui.exe"))
    parser.add_argument("--sidebar-only", action="store_true", help="check sidebar resizing and collapse only")
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.exit(2, "This check requires an interactive Windows desktop.\n")
    check(args.binary.resolve(), args.sidebar_only)
