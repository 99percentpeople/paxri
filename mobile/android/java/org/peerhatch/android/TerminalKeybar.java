package org.peerhatch.android;

import android.content.res.ColorStateList;
import android.graphics.Color;
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
    private final Button[] modifiers = new Button[3];
    private final float cornerRadius;

    TerminalKeybar(PeerHatchActivity activity, long session) {
        super(activity);
        this.activity = activity;
        this.session = session;
        cornerRadius = dp(controlRadius());
        setOrientation(VERTICAL);
        setGravity(Gravity.CENTER_VERTICAL);
        setMotionEventSplittingEnabled(true);
        row(new String[]{"Esc", "Tab", "Ctrl", "Alt", "Shift"});
        row(new String[]{"←", "↓", "↑", "→", "Enter", "Keys"});
    }

    private int dp(float value) { return Math.round(value * getResources().getDisplayMetrics().density); }

    private void row(String[] labels) {
        LinearLayout row = new LinearLayout(activity);
        row.setMotionEventSplittingEnabled(true);
        addView(row, new LayoutParams(LayoutParams.MATCH_PARENT, dp(48)));
        for (String label : labels) {
            int bit = label.equals("Ctrl") ? CTRL : label.equals("Alt") ? ALT : label.equals("Shift") ? SHIFT : 0;
            Button button = new Button(activity) {
                private boolean chord;

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
            button.setTextSize(14);
            button.setMinWidth(0);
            button.setMinimumWidth(0);
            button.setPadding(0, 0, 0, 0);
            button.setFocusable(false);
            button.setStateListAnimator(null);
            int[][] states = {{android.R.attr.state_selected}, {android.R.attr.state_pressed}, {}};
            GradientDrawable bg = new GradientDrawable();
            bg.setCornerRadius(cornerRadius);
            bg.setColor(new ColorStateList(states, new int[]{0xffe7e7e7, 0xff444444, 0xff262626}));
            button.setBackground(bg);
            button.setTextColor(new ColorStateList(states, new int[]{Color.BLACK, Color.WHITE, Color.WHITE}));
            LayoutParams params = new LayoutParams(0, LayoutParams.MATCH_PARENT, 1);
            params.setMargins(row.getChildCount() == 0 ? 0 : dp(4), dp(2), 0, dp(2));
            row.addView(button, params);
            if (bit == 0) {
                button.setOnClickListener(v -> activity.terminalAction(session, label));
                continue;
            }
            modifiers[Integer.numberOfTrailingZeros(bit)] = button;
            // Accessibility clicks also provide the one-shot latch.
            button.setOnClickListener(v -> { latched ^= bit; refresh(); });
            button.setOnTouchListener((v, e) -> {
                switch (e.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    activity.finishTerminalComposition();
                    held |= bit;
                    used &= ~bit;
                    break;
                case MotionEvent.ACTION_UP:
                    // Some IMEs leave the letter composing until the chord is released.
                    activity.finishTerminalComposition();
                    held &= ~bit;
                    if ((used & bit) == 0 && e.getX() >= 0 && e.getX() < v.getWidth()
                            && e.getY() >= 0 && e.getY() < v.getHeight()
                            && SystemClock.uptimeMillis() - e.getDownTime() < ViewConfiguration.getLongPressTimeout()) {
                        v.performClick();
                    }
                    break;
                case MotionEvent.ACTION_CANCEL:
                    held &= ~bit;
                    latched &= ~bit;
                    break;
                default: return true;
                }
                refresh();
                return true;
            });
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
        for (int i = 0; i < modifiers.length; i++) {
            if (modifiers[i] != null) modifiers[i].setSelected(((held | latched) & (1 << i)) != 0);
        }
    }
}
