package org.paxri.android;

import android.content.Context;
import android.content.Intent;
import android.content.res.Configuration;
import android.graphics.Rect;
import android.graphics.Insets;
import android.database.ContentObserver;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.os.Bundle;
import android.text.InputType;
import android.util.DisplayMetrics;
import android.util.TypedValue;
import android.view.Display;
import android.view.Gravity;
import android.view.HapticFeedbackConstants;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.View;
import android.view.WindowManager;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.PopupWindow;
import android.widget.ScrollView;
import android.widget.LinearLayout;
import android.widget.Toast;
import dev.gpui.mobile.GpuiInputActivity;

public final class PaxriActivity extends GpuiInputActivity {
    private static final int SCAN_PAIRING = 71;
    private static final String DEVICE_NAME_OVERRIDE = "device_name";
    private static native void initVerifier(Context application);
    private static native void activityVisibilityChanged(boolean visible);
    private static native void deviceNameChanged(String name, String model);
    private static native void submitCode(String code);
    private static native void systemTextSizeChanged(float size);
    private static native void insetsChanged(float left, float top, float right, float bottom);
    private static native boolean goBack();
    private static native void terminalKey(long session, String value, boolean named, int first, int rest);
    private static native void composingTextChanged(long terminal, String text, int start, int end);
    private static native void startKeyboardSession();
    private long imeSession;
    private boolean networkReady;
    private Toast toast;
    private PopupWindow terminalTouch;
    private long touchSession;
    private boolean terminalTouchable = true;
    private TerminalGestures gestures;
    private TerminalKeybar keybar;
    private int keybarHeight;
    private String keyLayout;
    private TerminalKeybar.Colors keyColors;
    private ScrollView keybarScroll;
    private Rect touchArea = new Rect();
    private boolean visible;
    private static int startedActivities;
    private static PaxriActivity connectionServiceOwner;
    private boolean deviceConnected;
    private boolean connectionServiceStarted;
    private final Handler connectionHandler = new Handler(Looper.getMainLooper());
    private final Runnable stopConnections = this::stopConnectionService;
    private String scannedCode;
    private Insets previousInsets;
    private float insetDensity;
    private EditText terminalInput;
    private long terminalInputSession;
    private boolean terminalComposing;
    private android.media.Ringtone bellRingtone;

    private void clearTerminalComposition() {
        if (terminalComposing) {
            composingTextChanged(terminalInputSession, "", 0, 0);
            terminalComposing = false;
        }
    }

    private final ContentObserver nameObserver = new ContentObserver(new Handler(Looper.getMainLooper())) {
        @Override
        public void onChange(boolean selfChange) { publishDeviceName(); }
    };

    private native boolean terminalShortcut(long session, int modifiers, String key, boolean hasSelection);
    private final android.util.SparseBooleanArray shortcutKeys = new android.util.SparseBooleanArray();

    public float terminalZoom() {
        android.content.SharedPreferences prefs = getPreferences(MODE_PRIVATE);
        return prefs.getFloat("terminal_zoom", prefs.getFloat("window_zoom", 1f));
    }

    public void saveTerminalZoom(float scale) {
        getPreferences(MODE_PRIVATE).edit().putFloat("terminal_zoom", scale).apply();
    }

    @Override public boolean dispatchKeyEvent(KeyEvent event) {
        int code = event.getKeyCode();
        if (event.getAction() == KeyEvent.ACTION_UP && shortcutKeys.get(code)) {
            shortcutKeys.delete(code);
            return true;
        }
        if (event.getAction() == KeyEvent.ACTION_DOWN && visible && terminalTouchable && touchSession != 0
                && !event.isFunctionPressed()) {
            int mods = (event.isCtrlPressed() ? 1 : 0) | (event.isAltPressed() ? 2 : 0)
                    | (event.isShiftPressed() ? 4 : 0) | (event.isMetaPressed() ? 8 : 0);
            boolean functionKey = code >= KeyEvent.KEYCODE_F1 && code <= KeyEvent.KEYCODE_F12;
            if ((mods & ~4) != 0 || functionKey) {
                String key = shortcutKey(event, functionKey);
                // GPUI reports the shifted symbol itself on non-letter keys.
                if (key != null && key.length() == 1 && !Character.isLetter(key.charAt(0))
                        && key.charAt(0) != event.getUnicodeChar(0)) {
                    mods &= ~4;
                }
                if (key != null && terminalShortcut(touchSession, mods, key, gestures != null && gestures.hasSelection())) {
                    shortcutKeys.put(code, true);
                    return true;
                }
            }
        }
        return super.dispatchKeyEvent(event);
    }

    private static String shortcutKey(KeyEvent event, boolean functionKey) {
        int code = event.getKeyCode();
        if (functionKey) return "F" + (code - KeyEvent.KEYCODE_F1 + 1);
        switch (code) {
            case KeyEvent.KEYCODE_DPAD_UP: return "up";
            case KeyEvent.KEYCODE_DPAD_DOWN: return "down";
            case KeyEvent.KEYCODE_DPAD_LEFT: return "left";
            case KeyEvent.KEYCODE_DPAD_RIGHT: return "right";
            case KeyEvent.KEYCODE_MOVE_HOME: return "home";
            case KeyEvent.KEYCODE_MOVE_END: return "end";
            case KeyEvent.KEYCODE_PAGE_UP: return "pageup";
            case KeyEvent.KEYCODE_PAGE_DOWN: return "pagedown";
            case KeyEvent.KEYCODE_DEL: return "backspace";
            case KeyEvent.KEYCODE_FORWARD_DEL: return "delete";
            case KeyEvent.KEYCODE_INSERT: return "insert";
            case KeyEvent.KEYCODE_ENTER: case KeyEvent.KEYCODE_NUMPAD_ENTER: return "enter";
            case KeyEvent.KEYCODE_SPACE: return "space";
            case KeyEvent.KEYCODE_TAB: return "tab";
            case KeyEvent.KEYCODE_ESCAPE: return "escape";
            default:
                int symbol = event.getUnicodeChar(event.isShiftPressed() ? KeyEvent.META_SHIFT_ON : 0);
                return symbol > 32 && symbol < 127 ? Character.toString((char)symbol) : null;
        }
    }

    private String systemDeviceName() {
        try {
            String setting = Settings.Global.getString(getContentResolver(), Settings.Global.DEVICE_NAME);
            if (setting != null && !setting.trim().isEmpty()) return setting;
        } catch (SecurityException ignored) {
            // Some vendors restrict access to the system device name.
        }
        return Build.MODEL == null || Build.MODEL.isEmpty() ? "Android" : Build.MODEL;
    }

    private void publishDeviceName() {
        String name = getPreferences(MODE_PRIVATE).getString(DEVICE_NAME_OVERRIDE, "");
        deviceNameChanged(name, systemDeviceName());
    }

    public void saveDeviceName(String name) {
        android.content.SharedPreferences.Editor editor = getPreferences(MODE_PRIVATE).edit();
        if (name.isEmpty()) editor.remove(DEVICE_NAME_OVERRIDE);
        else editor.putString(DEVICE_NAME_OVERRIDE, name);
        editor.apply();
        publishDeviceName();
    }

    public void setDeviceConnections(boolean paired, boolean connected) {
        runOnUiThread(() -> {
            if (isDestroyed() || isFinishing()) return;
            deviceConnected = connected;
            connectionHandler.removeCallbacks(stopConnections);
            if (connected) {
                startConnectionServiceIfVisible();
            } else if (!paired) {
                stopConnectionService();
            } else if (connectionServiceStarted) {
                // Keep the process eligible for reconnect during the gateway's offline grace period.
                connectionHandler.postDelayed(stopConnections, 5 * 60 * 1000L);
            }
        });
    }

    private void startConnectionServiceIfVisible() {
        if (!visible || !deviceConnected || connectionServiceStarted) return;
        try {
            startForegroundService(new Intent(this, ConnectionService.class));
            connectionServiceStarted = true;
            connectionServiceOwner = this;
        } catch (IllegalStateException | SecurityException error) {
            android.util.Log.w("paxri", "Starting background device connections", error);
        }
    }

    private void stopConnectionService() {
        if (connectionServiceStarted) {
            if (connectionServiceOwner == this) {
                stopService(new Intent(this, ConnectionService.class));
                connectionServiceOwner = null;
            }
            connectionServiceStarted = false;
        }
    }

    @Override
    protected void onDestroy() {
        connectionHandler.removeCallbacks(stopConnections);
        stopConnectionService();
        if (bellRingtone != null) {
            bellRingtone.stop();
            bellRingtone = null;
        }
        super.onDestroy();
    }

    @Override
    protected void onStart() {
        super.onStart();
        visible = true;
        startConnectionServiceIfVisible();
        updateTerminalTouch();
        if (++startedActivities == 1) activityVisibilityChanged(true);
        try {
            getContentResolver().registerContentObserver(
                    Settings.Global.getUriFor(Settings.Global.DEVICE_NAME), false, nameObserver);
        } catch (SecurityException ignored) {
            // Read the current name again when the user returns from Settings.
        }
        publishDeviceName();
    }

    @Override
    protected void onPause() {
        shortcutKeys.clear();
        if (keybar != null) keybar.cancelPendingInputEvents();
        super.onPause();
    }

    @Override
    protected void onStop() {
        visible = false;
        dismissTerminalTouch();
        if (--startedActivities == 0) activityVisibilityChanged(false);
        getContentResolver().unregisterContentObserver(nameObserver);
        super.onStop();
    }

    @Override
    protected void onCreate(Bundle state) {
        System.loadLibrary("paxri_android");
        reportSystemTextSize();
        initVerifier(getApplicationContext());
        publishDeviceName();
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        if (state != null) scannedCode = state.getString("scannedCode");
        super.onCreate(state);
        SystemBars.install(this, this::reportInsets);
    }

    @Override
    public void onAttachedToWindow() {
        super.onAttachedToWindow();
        if (Build.VERSION.SDK_INT >= 30) {
            // GPUI samples these native touch positions on the display frame.
            getWindow().getDecorView().requestUnbufferedDispatch(InputDevice.SOURCE_CLASS_POINTER);
        }
        // NativeActivity's input queue follows the window's refresh rate, even
        // when the Vulkan surface has already requested a higher rate.
        Display display = getWindowManager().getDefaultDisplay();
        Display.Mode current = display.getMode();
        Display.Mode preferred = current;
        float target = Math.max(120f, current.getRefreshRate());
        for (Display.Mode mode : display.getSupportedModes()) {
            if (mode.getPhysicalWidth() == current.getPhysicalWidth()
                    && mode.getPhysicalHeight() == current.getPhysicalHeight()
                    && Math.abs(mode.getRefreshRate() - target)
                        < Math.abs(preferred.getRefreshRate() - target)) {
                preferred = mode;
            }
        }
        if (Math.round(preferred.getRefreshRate()) > 60) {
            WindowManager.LayoutParams params = getWindow().getAttributes();
            params.preferredDisplayModeId = preferred.getModeId();
            getWindow().setAttributes(params);
        }
    }

    public void setChromeAppearance(boolean darkIcons, int panel, int key, int pressed, int text, int selectedText) {
        runOnUiThread(() -> {
            SystemBars.setDarkIcons(getWindow(), darkIcons);
            keyColors = new TerminalKeybar.Colors(panel, key, pressed, text, selectedText);
            if (keybar != null) keybar.applyColors(keyColors);
            else updateTerminalTouch();
        });
    }

    public void setTerminalTouchArea(long session, int x, int y, int width, int height, int toolbarHeight, String layout, boolean touchable) {
        runOnUiThread(() -> {
            if (touchSession != session) dismissTerminalTouch();
            if (width > 0 && keybarScroll != null && !layout.equals(keyLayout)) {
                keybarScroll.removeAllViews();
                keybar = new TerminalKeybar(this, session, layout, keyColors);
                keybarScroll.addView(keybar);
            }
            keyLayout = layout;
            touchSession = session;
            if (terminalTouchable && !touchable && keybar != null) keybar.cancelPendingInputEvents();
            terminalTouchable = touchable;
            keybarHeight = toolbarHeight;
            touchArea.set(x, y, x + width, y + height);
            updateTerminalTouch();
        });
    }

    public void setSelectionHandles(long session, int x1, int y1, int x2, int y2, int lineHeight, boolean connected) {
        runOnUiThread(() -> {
            if (touchSession == session && gestures != null)
                gestures.setHandles(x1, y1, x2, y2, lineHeight, connected);
        });
    }

    public void stopTerminalFling(long session) {
        runOnUiThread(() -> {
            if (touchSession == session && gestures != null) {
                gestures.stopFling();
            }
        });
    }

    private void dismissTerminalTouch() {
        if (terminalTouch != null) {
            keybar.cancelPendingInputEvents();
            terminalTouch.dismiss();
            terminalTouch = null;
            gestures = null;
            keybar = null;
            keybarScroll = null;
        }
    }

    private void updateTerminalTouch() {
        View decor = getWindow().getDecorView();
        // The settings preview uses only the keybar portion, without an active terminal.
        boolean preview = touchSession == 0 && keybarHeight > 0;
        if (!visible || keyColors == null || !decor.isAttachedToWindow() || touchArea.width() <= 0
                || (!preview && (touchSession == 0 || touchArea.isEmpty()))) {
            dismissTerminalTouch();
            return;
        }
        int[] origin = new int[2];
        decor.getLocationOnScreen(origin);
        int x = origin[0] + touchArea.left, y = origin[1] + touchArea.top;
        if (terminalTouch == null) {
            // NativeActivity sends its main surface straight to the NDK input queue.
            // Use an unfocusable child window over just the canvas so the IME stays with GPUI.
            keybar = new TerminalKeybar(this, touchSession, keyLayout, keyColors);
            keybarScroll = new ScrollView(this);
            keybarScroll.setVerticalScrollBarEnabled(false);
            keybarScroll.addView(keybar);
            LinearLayout panel = new LinearLayout(this);
            panel.setOrientation(LinearLayout.VERTICAL);
            panel.setMotionEventSplittingEnabled(true);
            if (!preview) {
                gestures = new TerminalGestures(this, touchSession);
                panel.addView(gestures, new LinearLayout.LayoutParams(-1, 0, 1));
            }
            panel.addView(keybarScroll, new LinearLayout.LayoutParams(-1, keybarHeight));
            terminalTouch = new PopupWindow(panel, touchArea.width(), touchArea.height() + keybarHeight, false);
            terminalTouch.setSplitTouchEnabled(true);
            terminalTouch.setTouchModal(false);
            terminalTouch.setTouchable(terminalTouchable);
            terminalTouch.setIsLaidOutInScreen(true);
            // Chinese IMEs can draw the unfinished spelling just above the keyboard.
            // Keep their window above the touch surface and shortcut keys.
            terminalTouch.setInputMethodMode(PopupWindow.INPUT_METHOD_NEEDED);
            terminalTouch.showAtLocation(decor, Gravity.TOP | Gravity.LEFT, x, y);
        } else {
            if (keybarScroll.getLayoutParams().height != keybarHeight) {
                keybarScroll.setLayoutParams(new LinearLayout.LayoutParams(-1, keybarHeight));
            }
            // Let GPUI receive touches on menus drawn over the native gesture surface.
            terminalTouch.setTouchable(terminalTouchable);
            terminalTouch.update(x, y, touchArea.width(), touchArea.height() + keybarHeight);
        }
    }

    @Override
    public void onConfigurationChanged(Configuration configuration) {
        super.onConfigurationChanged(configuration);
        reportSystemTextSize();
    }

    private void reportSystemTextSize() {
        DisplayMetrics metrics = getResources().getDisplayMetrics();
        float pixels = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, 16f, metrics);
        systemTextSizeChanged(pixels / metrics.density);
    }

    private void reportInsets(Insets safe) {
        float density = getResources().getDisplayMetrics().density;
        if (safe.equals(previousInsets) && density == insetDensity) return;
        previousInsets = safe;
        insetDensity = density;
        insetsChanged(safe.left / density, safe.top / density, safe.right / density, safe.bottom / density);
    }

    public void showTerminalKeyboard(long session) {
        runOnUiThread(() -> {
            if (touchSession != session) return;
            if (terminalInputSession == session && terminalInput != null && terminalInput.hasFocus()) {
                // Also reopens the same editor after the system Back gesture hides the IME.
                InputMethodManager imm = (InputMethodManager) getSystemService(INPUT_METHOD_SERVICE);
                imm.showSoftInput(terminalInput, InputMethodManager.SHOW_IMPLICIT);
            } else {
                startKeyboardSession();
            }
        });
    }

    @Override
    protected void configureIme(EditText field, long session) {
        clearTerminalComposition();
        imeSession = session;
        terminalInputSession = touchSession;
        terminalInput = touchSession != 0 ? field : null;
        if (touchSession != 0) {
            field.setRawInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_MULTI_LINE);
            field.setImeOptions(EditorInfo.IME_FLAG_NO_EXTRACT_UI | EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING);
        } else {
            int type = field.getInputType();
            field.setRawInputType((type & InputType.TYPE_MASK_CLASS) == InputType.TYPE_CLASS_NUMBER
                    ? type | InputType.TYPE_NUMBER_FLAG_SIGNED
                    : InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_CAP_WORDS);
            field.setImeOptions(EditorInfo.IME_ACTION_DONE | EditorInfo.IME_FLAG_NO_EXTRACT_UI);
        }
    }

    @Override
    public void gpuiHideKeyboard(long session) {
        runOnUiThread(() -> {
            clearTerminalComposition();
            terminalInputSession = 0;
            terminalInput = null;
            imeSession = session;
            super.gpuiHideKeyboard(session);
        });
    }

    void finishTerminalComposition() {
        if (getCurrentFocus() instanceof EditText) {
            EditText field = (EditText)getCurrentFocus();
            if (BaseInputConnection.getComposingSpanStart(field.getText()) >= 0) {
                InputConnection connection = field.onCreateInputConnection(new EditorInfo());
                if (connection != null) connection.finishComposingText();
            }
        }
    }

    void terminalAction(long session, String key) {
        if (session == 0 || touchSession != session || keybar == null) return;
        if (key.equals("Keys")) {
            TerminalGestures.keyboard(session);
            return;
        }
        finishTerminalComposition();
        sendTerminalInput(key, true);
    }

    private void sendTerminalInput(String value, boolean named) {
        if (keybar != null && !value.isEmpty()) {
            if (gestures != null) gestures.dismissSelection();
            int first = keybar.takeModifiers();
            terminalKey(touchSession, value, named, first, keybar.heldModifiers());
        }
    }

    public void terminalHardwareText(long session, String text) {
        runOnUiThread(() -> {
            if (touchSession != session) return;
            switch (text) {
            case "\u001b[D": sendTerminalInput("Left", true); break;
            case "\u001b[C": sendTerminalInput("Right", true); break;
            case "\u001b[A": sendTerminalInput("Up", true); break;
            case "\u001b[B": sendTerminalInput("Down", true); break;
            case "\u001b[H": sendTerminalInput("home", true); break;
            case "\u001b[F": sendTerminalInput("end", true); break;
            default: sendTerminalInput(text, false);
            }
        });
    }

    @Override protected void deliverIme(long session, int kind, String text, int start, int end) {
        if (session != imeSession || terminalInputSession != touchSession) return;
        if (touchSession == 0) {
            if (kind == 1 && "\n".equals(text)) kind = 4;
            super.deliverIme(session, kind, text, start, end);
            return;
        }
        if (kind == 0) {
            terminalComposing = !text.isEmpty();
            composingTextChanged(terminalInputSession, text, start, end);
            return;
        }
        if (kind == 1 || kind == 2 || kind == 3) {
            if (kind == 1) {
                clearTerminalComposition();
                sendTerminalInput(text, false);
            } else {
                for (int i = 0; i < start; i++) sendTerminalInput("backspace", true);
            }
            return;
        }
        super.deliverIme(session, kind, text, start, end);
    }

    @Override public void gpuiResetComposition(long session) {
        runOnUiThread(() -> {
            clearTerminalComposition();
            imeSession = session;
            super.gpuiResetComposition(session);
        });
    }

    public void showToast(String message) {
        runOnUiThread(() -> {
            if (isFinishing() || isDestroyed()) return;
            if (toast != null) toast.cancel();
            toast = Toast.makeText(getApplicationContext(), message, Toast.LENGTH_SHORT);
            toast.show();
        });
    }

    public void openUrl(String url) {
        runOnUiThread(() -> {
            if (isFinishing() || isDestroyed()) return;
            try {
                Intent intent = new Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url));
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                startActivity(intent);
            } catch (android.content.ActivityNotFoundException | SecurityException e) {
                showToast("No app can open this link");
            }
        });
    }

    public void terminalBell() {
        runOnUiThread(() -> {
            if (!visible || isFinishing() || isDestroyed()) return;
            android.media.AudioManager audio = (android.media.AudioManager) getSystemService(AUDIO_SERVICE);
            int mode = audio != null ? audio.getRingerMode() : android.media.AudioManager.RINGER_MODE_NORMAL;
            if (mode == android.media.AudioManager.RINGER_MODE_NORMAL) {
                try {
                    if (bellRingtone == null) {
                        android.net.Uri uri = android.media.RingtoneManager.getDefaultUri(
                                android.media.RingtoneManager.TYPE_NOTIFICATION);
                        if (uri != null) {
                            bellRingtone = android.media.RingtoneManager.getRingtone(
                                    getApplicationContext(), uri);
                        }
                    }
                    if (bellRingtone != null) {
                        if (bellRingtone.isPlaying()) bellRingtone.stop();
                        bellRingtone.play();
                    }
                } catch (RuntimeException e) {
                    android.util.Log.w("paxri", "Playing terminal bell", e);
                }
            } else if (mode == android.media.AudioManager.RINGER_MODE_VIBRATE) {
                View view = gestures != null ? gestures : getWindow().getDecorView();
                int feedback = Build.VERSION.SDK_INT >= 30
                        ? HapticFeedbackConstants.REJECT
                        : HapticFeedbackConstants.LONG_PRESS;
                view.performHapticFeedback(feedback);
            }
        });
    }

    public void scanPairingCode() {
        runOnUiThread(() -> startActivityForResult(new Intent(this, PairingScannerActivity.class), SCAN_PAIRING));
    }

    // Android can recreate this activity to deliver a result before the Rust client has started.
    public void pairingClientReady() {
        runOnUiThread(() -> {
            networkReady = true;
            deliverScan();
        });
    }

    private void deliverScan() {
        if (networkReady && scannedCode != null) {
            submitCode(scannedCode);
            scannedCode = null;
        }
    }

    @Override
    protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        if (scannedCode != null) state.putString("scannedCode", scannedCode);
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request == SCAN_PAIRING && result == RESULT_OK && data != null) {
            scannedCode = data.getStringExtra(PairingScannerActivity.CODE);
            deliverScan();
        }
    }

    private SystemBack systemBack;

    public void setBackEnabled(boolean enabled) {
        if (Build.VERSION.SDK_INT >= 33) {
            runOnUiThread(() -> {
                if (enabled && systemBack == null) {
                    systemBack = new SystemBack(this);
                } else if (!enabled && systemBack != null) {
                    systemBack.unregister();
                    systemBack = null;
                }
            });
        }
    }

    @android.annotation.TargetApi(33)
    private static final class SystemBack {
        private final android.window.OnBackInvokedDispatcher dispatcher;
        private final android.window.OnBackInvokedCallback callback;

        SystemBack(PaxriActivity activity) {
            callback = activity::backFromTerminal;
            dispatcher = activity.getOnBackInvokedDispatcher();
            dispatcher.registerOnBackInvokedCallback(
                    android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT, callback);
        }

        void unregister() {
            dispatcher.unregisterOnBackInvokedCallback(callback);
        }
    }

    private boolean backFromTerminal() {
        return (gestures != null && gestures.dismissSelection()) || goBack();
    }

    @Override
    public void onBackPressed() {
        if (!backFromTerminal()) super.onBackPressed();
    }
}
