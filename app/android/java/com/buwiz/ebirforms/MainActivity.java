package com.buwiz.oss;

import android.app.Activity;
import android.os.Bundle;

import com.longbridge.nsh.BuwizShim;
import com.longbridge.nsh.NshView;

/**
 * Buwiz's activity: nsh's AppActivity (which is final) as Buwiz needs it,
 * plus a hand to {@link Printing} so the Rust side can open the system
 * print dialog and the share sheet from this activity.
 */
public final class MainActivity extends Activity {
    private NshView view;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        BuwizShim.edgeToEdge(this);
        view = new NshView(this);
        BuwizShim.standalone(view);
        setContentView(view);
        Printing.attach(this);
        view.runRustApp("nsh");
    }

    @Override
    public void onConfigurationChanged(android.content.res.Configuration config) {
        super.onConfigurationChanged(config);
        BuwizShim.systemBars(this);
    }

    /** Android 12 and older: the app gets the back button first. */
    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        if (view == null || !view.handleBack()) super.onBackPressed();
    }

    @Override
    protected void onDestroy() {
        view.destroy();
        super.onDestroy();
    }
}
