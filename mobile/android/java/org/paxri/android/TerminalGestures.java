package org.paxri.android;

import android.app.Activity;
import android.content.ClipboardManager;
import android.content.res.TypedArray;
import android.graphics.Canvas;
import android.graphics.Rect;
import android.graphics.drawable.Drawable;
import android.view.ActionMode;
import android.view.GestureDetector;
import android.view.HapticFeedbackConstants;
import android.view.Menu;
import android.view.MenuItem;
import android.view.MotionEvent;
import android.view.ScaleGestureDetector;
import android.view.View;
import android.view.ViewConfiguration;
import android.widget.OverScroller;

// GPUI paints the terminal. This transparent input surface lets Android arbitrate
// one-finger drags, flings and pinches before any of them becomes a tap.
final class TerminalGestures extends View {
    static native void keyboard(long session);
    private static native void tap(long session, float x, float y);
    private static native void select(long session, float x, float y, boolean start);
    private static native boolean wantsAppDrag(long session);
    private static native void appDrag(long session, int action, float x, float y);
    private static native void moveHandle(long session, float x, float y, boolean leading);
    private static native void finishSelection(long session, boolean copy);
    private static native void paste(long session);
    private static native void scroll(long session, float dy, float x, float y, boolean fromFling);
    private static native void zoom(long session, float factor, boolean finished);

    private final long session;
    private final float density;
    private final GestureDetector drag;
    private final ScaleGestureDetector pinch;
    private final OverScroller fling;
    private final int minimumFlingVelocity;
    private final int touchSlop;
    private ActionMode selectionMode;
    private boolean connected;
    private boolean selecting;
    private boolean selectionDragging;
    private boolean scrolling;
    private boolean reportingDrag;
    private boolean multiplePointers;
    private boolean consumeTap;
    private int lastY;
    private float touchX, touchY;
    private final Handle leading, trailing;
    private Handle dragged;
    private int dragPointer;
    private float grabX, grabY;
    private int lineHeight;

    private final class Handle {
        final Drawable shape;
        final boolean first;
        int x, y;
        boolean flipped;

        Handle(Drawable drawable, boolean first) {
            shape = drawable;
            this.first = first;
        }

        void place(int x, int y) {
            this.x = x;
            this.y = y;
            if (shape == null) return;
            int width = shape.getIntrinsicWidth(), height = shape.getIntrinsicHeight();
            // At the outer edge, turn the handle inward and keep its tip on the cell boundary.
            flipped = (first ? x : getWidth() - x) < 12 * density;
            int left = x - Math.round(width * (first != flipped ? .75f : .25f));
            int top = Math.max(0, Math.min(getHeight() - height, y));
            shape.setBounds(left, top, left + width, top + height);
        }

        boolean visible() { return shape != null && lineHeight > 0 && y > 0 && y - lineHeight < getHeight(); }

        void draw(Canvas canvas) {
            if (!visible()) return;
            canvas.save();
            if (flipped) canvas.scale(-1f, 1f, shape.getBounds().exactCenterX(), 0f);
            shape.draw(canvas);
            canvas.restore();
        }

        float distance(float x, float y) {
            if (!visible()) return Float.POSITIVE_INFINITY;
            Rect b = shape.getBounds();
            int pad = Math.round(5 * density);
            if (x < b.left - pad || x > b.right + pad || y < b.top - pad || y > b.bottom + pad) return Float.POSITIVE_INFINITY;
            return (float)Math.hypot(x - b.exactCenterX(), y - b.exactCenterY());
        }
    }

    TerminalGestures(Activity context, long session) {
        super(context);
        this.session = session;
        density = getResources().getDisplayMetrics().density;
        TypedArray theme = context.obtainStyledAttributes(new int[]{
                android.R.attr.textSelectHandleLeft, android.R.attr.textSelectHandleRight});
        leading = new Handle(theme.getDrawable(0), true);
        trailing = new Handle(theme.getDrawable(1), false);
        theme.recycle();
        ViewConfiguration config = ViewConfiguration.get(context);
        minimumFlingVelocity = config.getScaledMinimumFlingVelocity();
        touchSlop = config.getScaledTouchSlop();
        fling = new OverScroller(context);
        drag = new GestureDetector(context, new GestureDetector.SimpleOnGestureListener() {
            @Override public boolean onDown(MotionEvent e) { return true; }

            @Override public boolean onSingleTapUp(MotionEvent e) {
                if (!consumeTap) tap(session, e.getX() / density, e.getY() / density);
                return true;
            }

            @Override public void onLongPress(MotionEvent e) {
                if (multiplePointers || !isAttachedToWindow()) return;
                consumeTap = true;
                selecting = true;
                selectionDragging = false;
                touchX = e.getX() / density;
                touchY = e.getY() / density;
                performHapticFeedback(HapticFeedbackConstants.LONG_PRESS);
                select(session, touchX, touchY, true);
            }

            @Override public boolean onScroll(MotionEvent start, MotionEvent e, float dx, float dy) {
                if (selecting || reportingDrag) return true;
                scrolling = true;
                scroll(session, -dy / density, e.getX() / density, e.getY() / density, false);
                touchY = e.getY() / density;
                touchX = e.getX() / density;
                return true;
            }

            @Override public boolean onFling(MotionEvent start, MotionEvent e, float vx, float vy) {
                if (!scrolling || Math.abs(vy) < minimumFlingVelocity) return false;
                lastY = 0;
                fling.fling(0, 0, 0, (int)vy, 0, 0, Integer.MIN_VALUE, Integer.MAX_VALUE);
                postInvalidateOnAnimation();
                return true;
            }
        });
        pinch = new ScaleGestureDetector(context, new ScaleGestureDetector.SimpleOnScaleGestureListener() {
            @Override public boolean onScale(ScaleGestureDetector detector) {
                zoom(session, detector.getScaleFactor(), false);
                return true;
            }
            @Override public void onScaleEnd(ScaleGestureDetector detector) {
                zoom(session, 1f, true);
            }
        });
        pinch.setQuickScaleEnabled(false);
        pinch.setStylusScaleEnabled(false);
    }

    @Override public boolean onTouchEvent(MotionEvent e) {
        int action = e.getActionMasked();
        if (action == MotionEvent.ACTION_DOWN && selectionMode != null) {
            float a = leading.distance(e.getX(), e.getY()), b = trailing.distance(e.getX(), e.getY());
            if (Math.min(a, b) < Float.POSITIVE_INFINITY) {
                dragged = a <= b ? leading : trailing;
                dragPointer = e.getPointerId(0);
                grabX = dragged.x - e.getX();
                grabY = dragged.y - lineHeight * .5f - e.getY();
                selectionMode.hide(ActionMode.DEFAULT_HIDE_DURATION);
                return true;
            }
        }
        if (dragged != null) {
            int index = e.findPointerIndex(dragPointer);
            if (index >= 0 && (action == MotionEvent.ACTION_MOVE || action == MotionEvent.ACTION_UP)) {
                touchX = (e.getX(index) + grabX) / density;
                touchY = (e.getY(index) + grabY) / density;
                moveHandle(session, touchX, touchY, dragged.first);
            }
            if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) {
                dragged = null;
                if (selectionMode != null) {
                    selectionMode.hide(0);
                    selectionMode.invalidateContentRect();
                }
            } else if (selectionMode != null) selectionMode.hide(ActionMode.DEFAULT_HIDE_DURATION);
            return true;
        }
        if (action == MotionEvent.ACTION_DOWN) {
            consumeTap = dismissSelection() || !fling.isFinished();
            fling.forceFinished(true);
            multiplePointers = false;
            scrolling = false;
            touchX = e.getX() / density;
            touchY = e.getY() / density;
        }
        if (action == MotionEvent.ACTION_POINTER_DOWN && !multiplePointers) {
            multiplePointers = true;
            finishAppDrag();
            cancelDragSelection();
            MotionEvent cancel = MotionEvent.obtain(e);
            cancel.setAction(MotionEvent.ACTION_CANCEL);
            drag.onTouchEvent(cancel);
            cancel.recycle();
        }
        if (selecting && (action == MotionEvent.ACTION_MOVE || action == MotionEvent.ACTION_UP)) {
            if (!selectionDragging) {
                selectionDragging = Math.hypot(e.getX() - touchX * density, e.getY() - touchY * density) > touchSlop;
                if (selectionDragging && wantsAppDrag(session)) {
                    reportingDrag = true;
                    appDrag(session, MotionEvent.ACTION_DOWN, touchX, touchY);
                }
            }
            if (selectionDragging) {
                touchX = e.getX() / density;
                touchY = e.getY() / density;
                if (reportingDrag) appDrag(session, MotionEvent.ACTION_MOVE, touchX, touchY);
                else select(session, touchX, touchY, false);
            }
            if (action == MotionEvent.ACTION_UP) {
                selecting = false;
                if (!reportingDrag) showSelectionMenu();
            }
        } else if (action == MotionEvent.ACTION_CANCEL) {
            cancelDragSelection();
        } else if (scrolling && !multiplePointers && action == MotionEvent.ACTION_UP) {
            float dy = e.getY() / density - touchY;
            if (dy != 0) scroll(session, dy, e.getX() / density, e.getY() / density, false);
        }
        pinch.onTouchEvent(e);
        if (!multiplePointers) drag.onTouchEvent(e);
        if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) finishAppDrag();
        return true;
    }

    @Override public boolean onGenericMotionEvent(MotionEvent e) {
        if (e.getActionMasked() != MotionEvent.ACTION_SCROLL) return super.onGenericMotionEvent(e);
        float pixels = e.getAxisValue(MotionEvent.AXIS_VSCROLL)
                * ViewConfiguration.get(getContext()).getScaledVerticalScrollFactor();
        scroll(session, pixels / density, e.getX() / density, e.getY() / density, false);
        return true;
    }

    @Override public void computeScroll() {
        if (!fling.computeScrollOffset()) return;
        int y = fling.getCurrY();
        if (y != lastY) scroll(session, (y - lastY) / density, touchX, touchY, true);
        lastY = y;
        postInvalidateOnAnimation();
    }

    void stopFling() { fling.forceFinished(true); }

    boolean hasSelection() { return lineHeight > 0; }

    void setHandles(int x1, int y1, int x2, int y2, int height, boolean connected) {
        boolean connectionChanged = this.connected != connected;
        this.connected = connected;
        lineHeight = height;
        leading.place(x1, y1);
        trailing.place(x2, y2);
        if (height == 0 && selectionMode != null) dismissSelection();
        else if (connectionChanged && selectionMode != null) selectionMode.invalidate();
        invalidate();
    }

    @Override protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        if ((selecting && !reportingDrag) || selectionMode != null) {
            leading.draw(canvas);
            trailing.draw(canvas);
        }
    }

    boolean dismissSelection() {
        if (selectionMode == null) return false;
        selectionMode.finish();
        return true;
    }

    private void finishAppDrag() {
        if (reportingDrag) {
            appDrag(session, MotionEvent.ACTION_UP, touchX, touchY);
            reportingDrag = false;
        }
    }

    private void cancelDragSelection() {
        if (selecting) {
            selecting = false;
            finishSelection(session, false);
        }
    }

    // hasText checks for nonempty text without retrieving the clipboard contents.
    @SuppressWarnings("deprecation")
    private boolean canPaste(ClipboardManager clipboard) {
        return connected && clipboard != null && clipboard.hasText();
    }

    private void showSelectionMenu() {
        PaxriActivity activity = (PaxriActivity)getContext();
        View decor = activity.getWindow().getDecorView();
        ClipboardManager clipboard = activity.getSystemService(ClipboardManager.class);
        selectionMode = decor.startActionMode(new ActionMode.Callback2() {
            private boolean copy;
            private final ClipboardManager.OnPrimaryClipChangedListener clipboardChanged = () -> {
                if (selectionMode != null) selectionMode.invalidate();
            };

            @Override public boolean onCreateActionMode(ActionMode mode, Menu menu) {
                menu.add(Menu.NONE, android.R.id.copy, Menu.NONE, android.R.string.copy)
                        .setShowAsAction(MenuItem.SHOW_AS_ACTION_ALWAYS);
                menu.add(Menu.NONE, android.R.id.paste, Menu.NONE, android.R.string.paste)
                        .setVisible(canPaste(clipboard)).setShowAsAction(MenuItem.SHOW_AS_ACTION_ALWAYS);
                if (clipboard != null) clipboard.addPrimaryClipChangedListener(clipboardChanged);
                return true;
            }

            @Override public boolean onPrepareActionMode(ActionMode mode, Menu menu) {
                MenuItem paste = menu.findItem(android.R.id.paste);
                boolean visible = canPaste(clipboard);
                if (paste.isVisible() == visible) return false;
                paste.setVisible(visible);
                return true;
            }

            @Override public void onGetContentRect(ActionMode mode, View view, Rect outRect) {
                int[] origin = new int[2], anchor = new int[2];
                view.getLocationOnScreen(origin);
                getLocationOnScreen(anchor);
                int x = anchor[0] - origin[0] + Math.round(touchX * density);
                int y = anchor[1] - origin[1] + Math.round(touchY * density);
                outRect.set(x, y, x + 1, y + 1);
            }

            @Override public boolean onActionItemClicked(ActionMode mode, MenuItem item) {
                if (item.getItemId() == android.R.id.copy) {
                    copy = true;
                } else if (item.getItemId() == android.R.id.paste) {
                    activity.finishTerminalComposition();
                    paste(session);
                } else {
                    return false;
                }
                mode.finish();
                return true;
            }

            @Override public void onDestroyActionMode(ActionMode mode) {
                if (clipboard != null) clipboard.removePrimaryClipChangedListener(clipboardChanged);
                selectionMode = null;
                dragged = null;
                invalidate();
                finishSelection(session, copy);
            }
        }, ActionMode.TYPE_FLOATING);
        if (selectionMode == null) finishSelection(session, false);
    }

    @Override protected void onDetachedFromWindow() {
        fling.forceFinished(true);
        finishAppDrag();
        cancelDragSelection();
        dismissSelection();
        super.onDetachedFromWindow();
    }
}
