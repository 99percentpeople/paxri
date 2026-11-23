package org.paxri.android;

import android.app.AlertDialog;
import android.content.Context;
import android.content.Intent;
import android.graphics.Rect;
import android.graphics.Insets;
import android.database.ContentObserver;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.os.Bundle;
import android.text.Editable;
import android.text.InputType;
import android.text.TextWatcher;
import android.view.Gravity;
import android.view.View;
import android.view.WindowManager;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.PopupWindow;
import android.widget.ScrollView;
import android.widget.LinearLayout;
import dev.gpui.mobile.GpuiInputActivity;

public final class PaxriActivity extends GpuiInputActivity {
    private static final int SCAN_PAIRING = 71;
    private static final String DEVICE_NAME_OVERRIDE = "device_name";
    private static native void initVerifier(Context application);
    private static native void activityVisibilityChanged(boolean visible);
    private static native void deviceNameChanged(String name, String model);
    private static native void submitCode(String code);
    private static native void insetsChanged(float left, float top, float right, float bottom);
    private static native boolean goBack();
    private static native void terminalKey(long session, String value, boolean named, int first, int rest);
    private static native void composingTextChanged(String text);
    private long imeSession;
    private boolean networkReady;
    private PopupWindow terminalTouch;
    private long touchSession;
    private TerminalGestures gestures;
    private TerminalKeybar keybar;
    private int keybarHeight;
    private String keyLayout;
    private TerminalKeybar.Colors keyColors;
    private ScrollView keybarScroll;
    private Rect touchArea = new Rect();
    private boolean visible;
    private String scannedCode;
    private Insets previousInsets;
    private float insetDensity;
    private EditText terminalInput;
    private String lastComposition = "";

    // Sample after the InputConnection batch, when composing spans are final.
    private final Runnable reportComposition = () -> {
        if (terminalInput == null) return;
        Editable text = terminalInput.getText();
        int start = BaseInputConnection.getComposingSpanStart(text);
        int end = BaseInputConnection.getComposingSpanEnd(text);
        String composing = start >= 0 && end >= start ? text.subSequence(start, end).toString() : "";
        if (!composing.equals(lastComposition)) {
            lastComposition = composing;
            composingTextChanged(composing);
        }
    };

    private final TextWatcher compositionWatcher = new TextWatcher() {
        public void beforeTextChanged(CharSequence s, int start, int count, int after) {}
        public void onTextChanged(CharSequence s, int start, int before, int count) {}
        public void afterTextChanged(Editable text) {
            terminalInput.removeCallbacks(reportComposition);
            terminalInput.post(reportComposition);
        }
    };

    private final ContentObserver nameObserver = new ContentObserver(new Handler(Looper.getMainLooper())) {
        @Override
        public void onChange(boolean selfChange) { publishDeviceName(); }
    };

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

    @Override
    protected void onStart() {
        super.onStart();
        visible = true;
        updateTerminalTouch();
        activityVisibilityChanged(true);
        try {
            getContentResolver().registerContentObserver(
                    Settings.Global.getUriFor(Settings.Global.DEVICE_NAME), false, nameObserver);
        } catch (SecurityException ignored) {
            // Read the current name again when the user returns from Settings.
        }
        publishDeviceName();
    }

    @Override
    protected void onStop() {
        visible = false;
        dismissTerminalTouch();
        activityVisibilityChanged(false);
        getContentResolver().unregisterContentObserver(nameObserver);
        super.onStop();
    }

    @Override
    protected void onCreate(Bundle state) {
        System.loadLibrary("paxri_android");
        initVerifier(getApplicationContext());
        publishDeviceName();
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        if (state != null) scannedCode = state.getString("scannedCode");
        super.onCreate(state);
        SystemBars.install(this, this::reportInsets);
    }

    public void setChromeAppearance(boolean darkIcons, int panel, int key, int pressed, int text, int selectedText) {
        runOnUiThread(() -> {
            SystemBars.setDarkIcons(getWindow(), darkIcons);
            keyColors = new TerminalKeybar.Colors(panel, key, pressed, text, selectedText);
            if (keybar != null) keybar.applyColors(keyColors);
            else updateTerminalTouch();
        });
    }

    public void setTerminalTouchArea(long session, int x, int y, int width, int height, int toolbarHeight, String layout) {
        runOnUiThread(() -> {
            if (touchSession != session) dismissTerminalTouch();
            if (width > 0 && keybarScroll != null && !layout.equals(keyLayout)) {
                keybarScroll.removeAllViews();
                keybar = new TerminalKeybar(this, session, layout, keyColors);
                keybarScroll.addView(keybar);
            }
            keyLayout = layout;
            touchSession = session;
            keybarHeight = toolbarHeight;
            touchArea.set(x, y, x + width, y + height);
            updateTerminalTouch();
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
            terminalTouch.setIsLaidOutInScreen(true);
            terminalTouch.setInputMethodMode(PopupWindow.INPUT_METHOD_NOT_NEEDED);
            terminalTouch.showAtLocation(decor, Gravity.TOP | Gravity.LEFT, x, y);
        } else {
            if (keybarScroll.getLayoutParams().height != keybarHeight) {
                keybarScroll.setLayoutParams(new LinearLayout.LayoutParams(-1, keybarHeight));
            }
            terminalTouch.update(x, y, touchArea.width(), touchArea.height() + keybarHeight);
        }
    }

    private void reportInsets(Insets safe) {
        float density = getResources().getDisplayMetrics().density;
        if (safe.equals(previousInsets) && density == insetDensity) return;
        previousInsets = safe;
        insetDensity = density;
        insetsChanged(safe.left / density, safe.top / density, safe.right / density, safe.bottom / density);
    }

    @Override
    public void gpuiShowKeyboard(int type, long session) {
        runOnUiThread(() -> {
            super.gpuiShowKeyboard(type, session);
            imeSession = session;
            if (getCurrentFocus() instanceof EditText) {
                EditText field = (EditText) getCurrentFocus();
                if (touchSession == 0) {
                    if (terminalInput != null) terminalInput.removeTextChangedListener(compositionWatcher);
                    terminalInput = null;
                    lastComposition = "";
                    int inputType = field.getInputType();
                    if ((inputType & InputType.TYPE_MASK_CLASS) == InputType.TYPE_CLASS_NUMBER) {
                        inputType |= InputType.TYPE_NUMBER_FLAG_SIGNED;
                    } else {
                        inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_CAP_WORDS;
                    }
                    field.setRawInputType(inputType);
                    field.setImeOptions(EditorInfo.IME_ACTION_DONE | EditorInfo.IME_FLAG_NO_EXTRACT_UI);
                    ((InputMethodManager) getSystemService(INPUT_METHOD_SERVICE)).restartInput(field);
                    return;
                }
                if (terminalInput != field) {
                    if (terminalInput != null) terminalInput.removeTextChangedListener(compositionWatcher);
                    terminalInput = field;
                    field.addTextChangedListener(compositionWatcher);
                }
                // Keep composition for CJK, but never autocorrect shell commands.
                field.setRawInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
                        | InputType.TYPE_TEXT_FLAG_MULTI_LINE | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
                field.setImeOptions(EditorInfo.IME_FLAG_NO_EXTRACT_UI | EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING);
                ((InputMethodManager) getSystemService(INPUT_METHOD_SERVICE)).restartInput(field);
            }
        });
    }

    @Override
    public void gpuiHideKeyboard(long session) {
        runOnUiThread(() -> {
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
            TerminalGestures.tap(session);
            return;
        }
        finishTerminalComposition();
        sendTerminalInput(key, true);
    }

    private void sendTerminalInput(String value, boolean named) {
        if (keybar != null && !value.isEmpty()) {
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
        if (session != imeSession) return;
        if (touchSession == 0) {
            if (kind == 1 && "\n".equals(text)) kind = 4;
            super.deliverIme(session, kind, text, start, end);
            return;
        }
        if (kind == 1 || kind == 2 || kind == 3) {
            if (keybar == null) return;
            if (kind == 1) {
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
            imeSession = session;
            super.gpuiResetComposition(session);
        });
    }

    public void requestPairingCode() {
        runOnUiThread(() -> {
            EditText field = new EditText(this);
            field.setSingleLine(true);
            field.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
            field.setHint("Paste pairing code");
            FrameLayout content = new FrameLayout(this);
            int pad = (int) (24 * getResources().getDisplayMetrics().density);
            content.setPadding(pad, 0, pad, 0);
            content.addView(field);
            AlertDialog dialog = new AlertDialog.Builder(this)
                    .setTitle("Pair a computer")
                    .setView(content)
                    .setNegativeButton("Cancel", null)
                    .setPositiveButton("Pair", (d, w) -> submitCode(field.getText().toString().trim()))
                    .create();
            dialog.setOnShowListener(d -> field.requestFocus());
            dialog.show();
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
        private final android.window.OnBackInvokedCallback callback = PaxriActivity::goBack;

        SystemBack(PaxriActivity activity) {
            dispatcher = activity.getOnBackInvokedDispatcher();
            dispatcher.registerOnBackInvokedCallback(
                    android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT, callback);
        }

        void unregister() {
            dispatcher.unregisterOnBackInvokedCallback(callback);
        }
    }

    @Override
    public void onBackPressed() {
        if (!goBack()) super.onBackPressed();
    }
}
