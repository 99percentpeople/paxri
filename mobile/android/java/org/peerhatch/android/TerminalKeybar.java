package org.peerhatch.android;

import android.content.res.ColorStateList;
import android.view.HapticFeedbackConstants;
import android.widget.HorizontalScrollView;
import android.widget.FrameLayout;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;
import java.util.ArrayList;
import java.util.HashMap;
import android.graphics.drawable.GradientDrawable;
import android.os.SystemClock;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.ViewConfiguration;
import android.view.accessibility.AccessibilityNodeInfo;
import android.widget.Button;
import android.widget.LinearLayout;

final class TerminalKeybar extends LinearLayout {
    private static native float controlRadius();
    static final int CTRL = 1, ALT = 2, SHIFT = 4;
    private final PeerHatchActivity activity;
    private final long session;
    private int held, latched, used;
    private final ArrayList<Button> buttons = new ArrayList<>();
    private final HashMap<Button, Integer> modifiers = new HashMap<>();
    private final int[] heldCounts = new int[3];
    private final float cornerRadius;
    private final int haptic;

    TerminalKeybar(PeerHatchActivity activity, long session, String config, Colors colors) {
        super(activity);
        this.activity = activity;
        this.session = session;
        cornerRadius = dp(controlRadius());
        setOrientation(VERTICAL);
        setGravity(Gravity.CENTER_VERTICAL);
        setMotionEventSplittingEnabled(true);
        try {
            JSONObject settings = new JSONObject(config);
            int rowHeight = settings.getInt("row_height");
            switch (settings.getString("haptics")) {
                case "off": haptic = -1; break;
                case "light": haptic = HapticFeedbackConstants.CLOCK_TICK; break;
                case "strong": haptic = HapticFeedbackConstants.LONG_PRESS; break;
                default: haptic = HapticFeedbackConstants.KEYBOARD_TAP;
            }
            JSONArray layout = settings.getJSONArray("rows");
            // Rows with the same column count share column widths, including wide labels.
            HashMap<Integer, int[]> widths = new HashMap<>();
            ArrayList<LinearLayout> rows = new ArrayList<>();
            for (int i = 0; i < layout.length(); i++) {
                JSONArray keys = layout.getJSONArray(i);
                int[] columns = widths.computeIfAbsent(keys.length(), count -> new int[count]);
                rows.add(row(keys, rowHeight, columns));
            }
            for (LinearLayout row : rows) {
                int[] columns = widths.get(row.getChildCount());
                for (int i = 0; i < columns.length; i++) {
                    row.getChildAt(i).getLayoutParams().width = columns[i];
                }
            }
        } catch (JSONException e) {
            throw new IllegalArgumentException("Invalid terminal key rows", e);
        }
        applyColors(colors);
    }

    private int dp(float value) { return Math.round(value * getResources().getDisplayMetrics().density); }

    private LinearLayout row(JSONArray keys, int rowHeight, int[] columns) throws JSONException {
        LinearLayout row = new LinearLayout(activity);
        row.setMotionEventSplittingEnabled(true);
        HorizontalScrollView strip = new HorizontalScrollView(activity);
        strip.setFillViewport(true);
        strip.setHorizontalScrollBarEnabled(false);
        strip.addView(row, new FrameLayout.LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.MATCH_PARENT));
        addView(strip, new LayoutParams(LayoutParams.MATCH_PARENT, dp(rowHeight)));
        for (int i = 0; i < keys.length(); i++) {
            JSONArray item = keys.getJSONArray(i);
            String key = item.getString(0), label = item.getString(1);
            int bit = label.equals("Ctrl") ? CTRL : label.equals("Alt") ? ALT : label.equals("Shift") ? SHIFT : 0;
            Button button = session == 0 ? new Button(activity) : new Button(activity) {
                private boolean chord;

                @Override public boolean dispatchTouchEvent(MotionEvent e) {
                    if (e.getActionMasked() == MotionEvent.ACTION_DOWN && haptic != -1) {
                        performHapticFeedback(haptic);
                    }
                    return super.dispatchTouchEvent(e);
                }

                @Override public boolean onTouchEvent(MotionEvent e) {
                    if (e.getActionMasked() == MotionEvent.ACTION_DOWN) {
                        chord = bit == 0 && held != 0 && !label.equals("Keys");
                        if (chord) {
                            setPressed(true);
                            performClick();
                        }
                    }
                    if (!chord) return super.onTouchEvent(e);
                    if (e.getActionMasked() == MotionEvent.ACTION_UP || e.getActionMasked() == MotionEvent.ACTION_CANCEL) {
                        setPressed(false);
                        chord = false;
                    }
                    return true;
                }

                @Override public void onInitializeAccessibilityNodeInfo(AccessibilityNodeInfo info) {
                    super.onInitializeAccessibilityNodeInfo(info);
                    if (bit != 0) {
                        info.setCheckable(true);
                        info.setChecked(isSelected());
                    }
                }
            };
            button.setAllCaps(false);
            button.setText(label);
            button.setSingleLine(true);
            button.setContentDescription(key);
            button.setTextSize(14);
            button.setMinWidth(0);
            button.setMinimumWidth(0);
            button.setPadding(0, 0, 0, 0);
            button.setFocusable(false);
            button.setStateListAnimator(null);
            buttons.add(button);
            int minWidth = Math.max(dp(38), (int)Math.ceil(button.getPaint().measureText(label)) + dp(10));
            columns[i] = Math.max(columns[i], minWidth);
            LayoutParams params = new LayoutParams(minWidth, LayoutParams.MATCH_PARENT, 1);
            params.setMargins(row.getChildCount() == 0 ? 0 : dp(4), dp(2), 0, dp(2));
            row.addView(button, params);
            if (session == 0) {
                button.setClickable(false);
                button.setLongClickable(false);
                continue;
            }
            if (bit == 0) {
                button.setOnClickListener(v -> activity.terminalAction(session, key));
                continue;
            }
            modifiers.put(button, bit);
            // Accessibility clicks also provide the one-shot latch.
            button.setOnClickListener(v -> { latched ^= bit; refresh(); });
            button.setOnTouchListener((v, e) -> {
                switch (e.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    activity.finishTerminalComposition();
                    if (heldCounts[Integer.numberOfTrailingZeros(bit)]++ == 0) {
                        held |= bit;
                        used &= ~bit;
                    }
                    button.setPressed(true);
                    break;
                case MotionEvent.ACTION_UP:
                    // Some IMEs leave the letter composing until the chord is released.
                    activity.finishTerminalComposition();
                    if (--heldCounts[Integer.numberOfTrailingZeros(bit)] == 0) held &= ~bit;
                    button.setPressed(false);
                    if ((held & bit) == 0 && (used & bit) == 0 && e.getX() >= 0 && e.getX() < v.getWidth()
                            && e.getY() >= 0 && e.getY() < v.getHeight()
                            && SystemClock.uptimeMillis() - e.getDownTime() < ViewConfiguration.getLongPressTimeout()) {
                        v.performClick();
                    }
                    break;
                case MotionEvent.ACTION_CANCEL:
                    if (--heldCounts[Integer.numberOfTrailingZeros(bit)] == 0) held &= ~bit;
                    button.setPressed(false);
                    latched &= ~bit;
                    break;
                default: return true;
                }
                refresh();
                return true;
            });
        }
        return row;
    }

    static final class Colors {
        final int panel;
        final ColorStateList fill, ink;

        Colors(int panel, int key, int pressed, int text, int selectedText) {
            this.panel = panel;
            int[][] states = {{android.R.attr.state_selected}, {android.R.attr.state_pressed}, {}};
            fill = new ColorStateList(states, new int[]{pressed, pressed, key});
            ink = new ColorStateList(states, new int[]{selectedText, text, text});
        }
    }

    private GradientDrawable surface(ColorStateList colors) {
        GradientDrawable shape = new GradientDrawable();
        shape.setCornerRadius(cornerRadius);
        shape.setColor(colors);
        return shape;
    }

    void applyColors(Colors colors) {
        setBackgroundColor(colors.panel);
        for (Button button : buttons) {
            button.setBackground(surface(colors.fill));
            button.setTextColor(colors.ink);
        }
    }

    int takeModifiers() {
        int active = held | latched;
        used |= held;
        latched = 0;
        refresh();
        return active;
    }

    int heldModifiers() { return held; }

    private void refresh() {
        for (Button button : modifiers.keySet()) {
            button.setSelected(((held | latched) & modifiers.get(button)) != 0);
        }
    }
}
