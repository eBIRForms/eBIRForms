package android.print;

import android.os.CancellationSignal;
import android.os.ParcelFileDescriptor;

import java.io.File;

/**
 * Buwiz's own file: drives a WebView's print adapter straight into a file
 * (the system dialog does the same when the filer picks Save as PDF). It
 * sits in {@code android.print} because the result callbacks' constructors
 * are not public.
 */
public final class BuwizPdfWriter {
    private BuwizPdfWriter() {}

    /** Lays out and writes every page of {@code adapter} to {@code out}; then {@code done}, or {@code failed} with why. */
    public static void write(final PrintDocumentAdapter adapter, PrintAttributes attrs, final File out,
                             final Runnable done, final java.util.function.Consumer<String> failed) {
        adapter.onStart();
        adapter.onLayout(null, attrs, new CancellationSignal(), new PrintDocumentAdapter.LayoutResultCallback() {
            @Override
            public void onLayoutFinished(PrintDocumentInfo info, boolean changed) {
                final ParcelFileDescriptor fd;
                try {
                    fd = ParcelFileDescriptor.open(out, ParcelFileDescriptor.MODE_CREATE
                            | ParcelFileDescriptor.MODE_TRUNCATE | ParcelFileDescriptor.MODE_READ_WRITE);
                } catch (Exception e) {
                    failed.accept(String.valueOf(e.getMessage()));
                    return;
                }
                adapter.onWrite(new PageRange[] {PageRange.ALL_PAGES}, fd, new CancellationSignal(),
                        new PrintDocumentAdapter.WriteResultCallback() {
                            @Override
                            public void onWriteFinished(PageRange[] pages) {
                                try { fd.close(); } catch (Exception ignored) {}
                                adapter.onFinish();
                                done.run();
                            }

                            @Override
                            public void onWriteFailed(CharSequence error) {
                                try { fd.close(); } catch (Exception ignored) {}
                                adapter.onFinish();
                                failed.accept(String.valueOf(error));
                            }
                        });
            }

            @Override
            public void onLayoutFailed(CharSequence error) {
                adapter.onFinish();
                failed.accept(String.valueOf(error));
            }
        }, null);
    }
}
