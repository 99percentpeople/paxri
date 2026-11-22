package org.peerhatch.android;

import android.content.Context;
import android.view.GestureDetector;
import android.view.MotionEvent;
import android.view.ScaleGestureDetector;
import android.view.View;
import android.view.ViewConfiguration;
import android.widget.OverScroller;

// GPUI paints the terminal. This transparent input surface lets Android arbitrate
// one-finger drags, flings and pinches before any of them becomes a tap.
final class TerminalGestures extends View {
    static native void tap(long session);
    private static native void scroll(long session, float dy, float x, float y, boolean fromFling);
    private static native void zoom(long session, float factor, boolean finished);

    private final long session;
    private final float density;
    private final GestureDetector drag;
    private final ScaleGestureDetector pinch;
    private final OverScroller fling;
    private final int minimumFlingVelocity;
    private float lastTouchY;
    private boolean multiplePointers;
    private boolean stoppedFling;
    private int lastY;
    private float touchX, touchY;

    TerminalGestures(Context context, long session) {
        super(context);
        this.session = session;
        density = getResources().getDisplayMetrics().density;
        minimumFlingVelocity = ViewConfiguration.get(context).getScaledMinimumFlingVelocity();
        fling = new OverScroller(context);
        drag = new GestureDetector(context, new GestureDetector.SimpleOnGestureListener() {
            @Override public boolean onDown(MotionEvent e) { return true; }

            @Override public boolean onSingleTapUp(MotionEvent e) {
                if (!stoppedFling) tap(session);
                return true;
            }

            @Override public boolean onFling(MotionEvent start, MotionEvent e, float vx, float vy) {
                if (Math.abs(vy) < minimumFlingVelocity) return false;
                lastY = 0;
                fling.fling(0, 0, 0, (int)vy, 0, 0, Integer.MIN_VALUE, Integer.MAX_VALUE);
                postInvalidateOnAnimation();
                return true;
            }
        });
        drag.setIsLongpressEnabled(false);
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
        if (action == MotionEvent.ACTION_DOWN) {
            stoppedFling = !fling.isFinished();
            fling.forceFinished(true);
            multiplePointers = false;
            lastTouchY = e.getY();
            touchX = e.getX() / density;
            touchY = e.getY() / density;
        }
        // Drag distance follows the finger even while Android is still deciding if this is a tap.
        if (!multiplePointers && (action == MotionEvent.ACTION_MOVE || action == MotionEvent.ACTION_UP)) {
            float y = e.getY();
            if (y != lastTouchY) scroll(session, (y - lastTouchY) / density, touchX, touchY, false);
            lastTouchY = y;
        }
        if (action == MotionEvent.ACTION_POINTER_DOWN && !multiplePointers) {
            multiplePointers = true;
            MotionEvent cancel = MotionEvent.obtain(e);
            cancel.setAction(MotionEvent.ACTION_CANCEL);
            drag.onTouchEvent(cancel);
            cancel.recycle();
        }
        pinch.onTouchEvent(e);
        if (!multiplePointers) drag.onTouchEvent(e);
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

    @Override protected void onDetachedFromWindow() {
        fling.forceFinished(true);
        super.onDetachedFromWindow();
    }
}
