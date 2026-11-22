package org.peerhatch.android;

import android.app.Activity;
import android.graphics.Color;
import android.graphics.Insets;
import android.graphics.Rect;
import android.os.Build;
import android.view.DisplayCutout;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;

import java.util.function.Consumer;

final class SystemBars {
    private SystemBars() {}

    static void install(Activity activity, Consumer<Insets> onSafeArea) {
        Window window = activity.getWindow();
        WindowManager.LayoutParams params = window.getAttributes();
        params.layoutInDisplayCutoutMode = Build.VERSION.SDK_INT >= 30
                ? WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
                : WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
        window.setAttributes(params);
        window.setStatusBarColor(Color.TRANSPARENT);
        window.setNavigationBarColor(Color.TRANSPARENT);
        window.setStatusBarContrastEnforced(false);
        window.setNavigationBarContrastEnforced(false);
        View decor = window.getDecorView();
        if (Build.VERSION.SDK_INT >= 30) {
            window.setDecorFitsSystemWindows(false);
            window.getInsetsController().show(WindowInsets.Type.systemBars());
        } else {
            // These flags only extend the layout beneath the visible system bars.
            decor.setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);
        }
        decor.setOnApplyWindowInsetsListener((view, insets) -> {
            onSafeArea.accept(safeArea(view, insets));
            return view.onApplyWindowInsets(insets);
        });
        decor.requestApplyInsets();
    }

    static void setDarkIcons(Window window, boolean dark) {
        if (Build.VERSION.SDK_INT >= 30) {
            int mask = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                    | WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;
            window.getInsetsController().setSystemBarsAppearance(dark ? mask : 0, mask);
        } else {
            View decor = window.getDecorView();
            int mask = View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR | View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR;
            int flags = decor.getSystemUiVisibility();
            decor.setSystemUiVisibility(dark ? flags | mask : flags & ~mask);
        }
    }

    private static Insets safeArea(View view, WindowInsets insets) {
        if (Build.VERSION.SDK_INT >= 30) {
            return insets.getInsets(WindowInsets.Type.displayCutout() | WindowInsets.Type.systemGestures()
                    | WindowInsets.Type.systemBars() | WindowInsets.Type.ime());
        }
        Insets safe = Insets.max(insets.getSystemGestureInsets(), insets.getSystemWindowInsets());
        DisplayCutout cutout = insets.getDisplayCutout();
        if (cutout != null) {
            safe = Insets.max(safe, Insets.of(cutout.getSafeInsetLeft(), cutout.getSafeInsetTop(),
                    cutout.getSafeInsetRight(), cutout.getSafeInsetBottom()));
        }
        // Android 10 exposes the IME through the visible display frame.
        Rect visible = new Rect();
        int[] origin = new int[2];
        view.getWindowVisibleDisplayFrame(visible);
        view.getLocationOnScreen(origin);
        return Insets.max(safe, Insets.of(0, 0, 0, origin[1] + view.getHeight() - visible.bottom));
    }
}
