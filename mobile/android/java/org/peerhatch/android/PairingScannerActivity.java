package org.peerhatch.android;

import android.Manifest;
import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.graphics.ImageDecoder;
import android.net.Uri;
import android.os.Bundle;
import android.view.Gravity;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.LinearLayout;

import com.google.zxing.BarcodeFormat;
import com.google.zxing.RGBLuminanceSource;
import com.google.zxing.Result;
import com.journeyapps.barcodescanner.CameraPreview;
import com.journeyapps.barcodescanner.Decoder;
import com.journeyapps.barcodescanner.DecoratedBarcodeView;
import com.journeyapps.barcodescanner.DefaultDecoderFactory;

import java.io.IOException;
import java.util.Collections;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public final class PairingScannerActivity extends Activity {
    static final String CODE = "org.peerhatch.android.pairingCode";
    private static final int CAMERA = 1;
    private static final int IMAGE = 2;
    private static final String PROMPT = "Fit the PeerHatch QR code inside the frame.";
    private static final String NO_CAMERA = "Camera unavailable. Choose a QR image or return to enter the code.";
    private static native boolean isInvitation(String code);
    static { System.loadLibrary("peerhatch_android"); }

    private final DefaultDecoderFactory qr = new DefaultDecoderFactory(
            Collections.singleton(BarcodeFormat.QR_CODE), null, null, 2);
    private final ExecutorService images = Executors.newSingleThreadExecutor();
    private DecoratedBarcodeView scanner;
    private boolean resumed;
    private boolean preview;
    private Uri reading;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        LinearLayout body = new LinearLayout(this);
        body.setOrientation(LinearLayout.VERTICAL);
        scanner = new DecoratedBarcodeView(this);
        scanner.setDecoderFactory(qr);
        scanner.setStatusText(PROMPT);
        scanner.decodeContinuous(result -> finishWithCode(result.getText()));
        scanner.getBarcodeView().addStateListener(new CameraPreview.StateListener() {
            public void previewSized() {}
            public void previewStarted() {}
            public void previewStopped() {}
            public void cameraClosed() {}
            public void cameraError(Exception e) { scanner.setStatusText(NO_CAMERA); }
        });
        body.addView(scanner, new LinearLayout.LayoutParams(-1, 0, 1));

        LinearLayout actions = new LinearLayout(this);
        actions.setGravity(Gravity.CENTER);
        Button choose = new Button(this);
        choose.setText("Choose QR image");
        choose.setOnClickListener(v -> {
            Intent pick = new Intent(Intent.ACTION_OPEN_DOCUMENT).setType("image/*")
                    .addCategory(Intent.CATEGORY_OPENABLE);
            startActivityForResult(pick, IMAGE);
        });
        Button cancel = new Button(this);
        cancel.setText("Cancel");
        cancel.setOnClickListener(v -> finish());
        actions.addView(choose);
        actions.addView(cancel);
        body.addView(actions);
        setContentView(body);
        SystemBars.install(this, safe -> body.setPadding(safe.left, safe.top, safe.right, safe.bottom));

        if (state != null && state.getParcelable("reading") != null) {
            readImage(state.getParcelable("reading"));
        } else if (!hasCameraPermission()) {
            if (state == null) requestPermissions(new String[]{Manifest.permission.CAMERA}, CAMERA);
            else scanner.setStatusText(NO_CAMERA);
        }
    }

    private boolean hasCameraPermission() {
        return checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED;
    }

    @Override
    public void onRequestPermissionsResult(int request, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(request, permissions, results);
        if (request == CAMERA) {
            if (!hasCameraPermission()) scanner.setStatusText(NO_CAMERA);
            startPreview();
        }
    }

    private void startPreview() {
        if (resumed && !preview && !isFinishing() && reading == null && hasCameraPermission()) {
            preview = true;
            scanner.resume();
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        resumed = true;
        startPreview();
    }

    @Override
    protected void onPause() {
        resumed = false;
        if (preview) scanner.pause();
        preview = false;
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        images.shutdownNow();
        super.onDestroy();
    }

    @Override
    protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        state.putParcelable("reading", reading);
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request == IMAGE && result == RESULT_OK && data != null && data.getData() != null) {
            readImage(data.getData());
        }
    }

    private void readImage(Uri uri) {
        reading = uri;
        scanner.setStatusText("Reading QR image…");
        images.execute(() -> {
            Result result = null;
            String error = "No QR code found. Choose another image or scan with the camera.";
            try {
                result = decodeImage(uri);
            } catch (IOException | RuntimeException e) {
                error = "Could not read this image. Choose another QR image.";
            }
            String text = result == null ? null : result.getText();
            String message = error;
            runOnUiThread(() -> {
                if (isDestroyed() || isFinishing() || !uri.equals(reading)) return;
                reading = null;
                if (text != null) finishWithCode(text);
                else scanner.setStatusText(message);
                startPreview();
            });
        });
    }

    private Result decodeImage(Uri uri) throws IOException {
        Bitmap image = ImageDecoder.decodeBitmap(ImageDecoder.createSource(getContentResolver(), uri),
                (decoder, info, source) -> {
                    int largest = Math.max(info.getSize().getWidth(), info.getSize().getHeight());
                    decoder.setTargetSampleSize(Math.max(1, (int) Math.ceil(largest / 2048.0)));
                    decoder.setAllocator(ImageDecoder.ALLOCATOR_SOFTWARE);
                });
        int width = image.getWidth(), height = image.getHeight();
        int[] pixels = new int[width * height];
        image.getPixels(pixels, 0, width, 0, 0, width, height);
        image.recycle();
        RGBLuminanceSource source = new RGBLuminanceSource(width, height, pixels);
        Decoder decoder = new DefaultDecoderFactory(Collections.singleton(BarcodeFormat.QR_CODE))
                .createDecoder(Collections.emptyMap());
        Result result = decoder.decode(source);
        return result == null ? decoder.decode(source.invert()) : result;
    }

    private void finishWithCode(String text) {
        if (isFinishing() || reading != null) return;
        if (!isInvitation(text)) {
            scanner.setStatusText("This is not a PeerHatch pairing QR code. Try another code.");
            return;
        }
        setResult(RESULT_OK, new Intent().putExtra(CODE, text));
        finish();
    }
}
