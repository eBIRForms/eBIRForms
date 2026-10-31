package com.longbridge.nsh;

import android.app.Activity;

/**
 * Buwiz's own file (not part of nsh): reaches the package-private setup
 * nsh's AppActivity does, so {@code com.buwiz.ebirforms.MainActivity} can do the
 * same.
 */
public final class BuwizShim {
    private BuwizShim() {}

    public static void edgeToEdge(Activity activity) {
        DevActivity.edgeToEdge(activity);
    }

    public static void systemBars(Activity activity) {
        DevActivity.systemBars(activity);
    }

    public static void standalone(NshView view) {
        view.useStandaloneLimits();
    }
}
