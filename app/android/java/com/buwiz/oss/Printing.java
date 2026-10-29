package com.buwiz.oss;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.print.BuwizPdfWriter;
import android.print.PrintAttributes;
import android.print.PrintManager;
import android.webkit.WebView;
import android.webkit.WebViewClient;

import java.io.File;
import java.io.FileOutputStream;

/**
 * Printing for Buwiz's Rust side (called through JNI from any thread; the
 * work runs on the UI thread): the system print dialog over a WebView's
 * print adapter (it offers Save as PDF), a PDF of the same pages written to
 * a file, and the share sheet for a PDF in the app's cache.
 */
public final class Printing {
    static {
        System.loadLibrary("nsh");
    }

    private Printing() {}

    /** The web view being printed, kept until the print job has its pages. */
    private static WebView keep;

    private static native void nativeAttach(Activity activity);

    static void attach(Activity activity) {
        nativeAttach(activity);
    }

    private static PrintAttributes a4() {
        return new PrintAttributes.Builder()
                .setMediaSize(PrintAttributes.MediaSize.ISO_A4)
                .setResolution(new PrintAttributes.Resolution("pdf", "pdf", 600, 600))
                .setMinMargins(PrintAttributes.Margins.NO_MARGINS)
                .build();
    }

    /** Loads {@code html} in a web view, then runs {@code ready} with it on the UI thread. */
    private static void load(final Activity activity, final String html, final java.util.function.Consumer<WebView> ready) {
        activity.runOnUiThread(() -> {
            final WebView web = new WebView(activity);
            web.setWebViewClient(new WebViewClient() {
                private boolean once;

                @Override
                public void onPageFinished(WebView view, String url) {
                    if (once) return;
                    once = true;
                    ready.accept(view);
                }
            });
            keep = web;
            web.loadDataWithBaseURL(null, html, "text/html", "UTF-8", null);
        });
    }

    /** The system print dialog for {@code html}, A4, no margins. */
    public static void print(final Activity activity, final String html, final String title) {
        load(activity, html, web -> {
            PrintManager manager = (PrintManager) activity.getSystemService(Context.PRINT_SERVICE);
            manager.print(title, web.createPrintDocumentAdapter(title), a4());
        });
    }

    /** Writes {@code html}'s pages as a PDF to {@code path}; then {@code path.done}, or {@code path.fail} holding why. */
    public static void renderPdf(final Activity activity, final String html, final String path) {
        load(activity, html, web -> {
            final File out = new File(path);
            BuwizPdfWriter.write(web.createPrintDocumentAdapter("Buwiz"), a4(), out,
                    () -> mark(path + ".done", ""),
                    why -> mark(path + ".fail", why));
        });
    }

    private static void mark(String path, String text) {
        try (FileOutputStream f = new FileOutputStream(path)) {
            f.write(text.getBytes("UTF-8"));
        } catch (Exception ignored) {
        }
    }

    /** The share sheet for the PDF {@code name} in the cache's {@code pdf} folder ({@link PdfFiles}). */
    public static void share(final Activity activity, final String name, final String title) {
        activity.runOnUiThread(() -> {
            Uri uri = Uri.parse("content://" + PdfFiles.AUTHORITY + "/" + Uri.encode(name));
            Intent send = new Intent(Intent.ACTION_SEND);
            send.setType("application/pdf");
            send.putExtra(Intent.EXTRA_STREAM, uri);
            send.putExtra(Intent.EXTRA_SUBJECT, title);
            send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
            Intent chooser = Intent.createChooser(send, title);
            chooser.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
            activity.startActivity(chooser);
        });
    }
}
