package org.paxri.android;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.net.ConnectivityManager;
import android.net.LinkAddress;
import android.net.LinkProperties;
import android.net.Network;
import android.net.wifi.WifiManager;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.PowerManager;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Foreground host for the embedded mobile gateway.
 *
 * Mirrors the desktop paxri-gateway lifecycle inside the Android process: keeps
 * the transport alive across app switches, screen-off, and task removal while
 * background_service is enabled, monitors OS network switches, and offers a
 * direct Stop control from the ongoing notification.
 */
public final class ConnectionService extends Service {
    private static final String CHANNEL = "gateway_service_silent";
    private static final String LEGACY_CHANNEL = "device_connections";
    private static final String ACTION_STOP = "org.paxri.android.action.STOP_GATEWAY";
    private static final int NOTIFICATION_ID = 1;
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static final Object LOCK = new Object();

    private static Context appContext;
    private static ConnectionService instance;
    private static boolean activityVisible;
    private static boolean serviceStartRequested;
    private static boolean gatewayRunning;
    private static boolean backgroundService = true;
    private static boolean screenOffKeepAlive = true;
    private static int connectedCount;
    private static int pairedCount;

    private static native void networkChanged();
    private static native void stopRequested();
    private static native void taskRemoved();

    private NotificationManager notifications;
    private ConnectivityManager connectivity;
    private ConnectivityManager.NetworkCallback networkCallback;
    private PowerManager.WakeLock wakeLock;
    private WifiManager.WifiLock wifiLock;
    private Network activeNetwork;
    private String activeLinkSignature;

    public static void init(Context context) {
        synchronized (LOCK) {
            appContext = context.getApplicationContext();
        }
    }

    public static void setActivityVisible(PaxriActivity activity, boolean visible) {
        synchronized (LOCK) {
            activityVisible = visible;
            if (visible) {
                appContext = activity.getApplicationContext();
            }
        }
        if (visible) {
            MAIN.post(ConnectionService::reconcileOnMainThread);
        }
    }

    public static void syncGatewayState(
            boolean running,
            boolean bgService,
            boolean screenOff,
            int connected,
            int paired) {
        synchronized (LOCK) {
            gatewayRunning = running;
            backgroundService = bgService;
            screenOffKeepAlive = screenOff;
            connectedCount = Math.max(0, connected);
            pairedCount = Math.max(0, paired);
        }
        MAIN.post(ConnectionService::reconcileOnMainThread);
    }

    private static void reconcileOnMainThread() {
        ConnectionService active;
        Context ctx;
        boolean running;
        boolean bgService;
        boolean visible;
        boolean shouldStart = false;
        synchronized (LOCK) {
            active = instance;
            ctx = appContext;
            running = gatewayRunning;
            bgService = backgroundService;
            visible = activityVisible;
            if (!running || !bgService) {
                serviceStartRequested = false;
            } else if (active == null && visible && ctx != null && !serviceStartRequested) {
                serviceStartRequested = true;
                shouldStart = true;
            }
        }
        if (!running || !bgService) {
            if (active != null) {
                active.releaseLocks();
                active.stopSelf();
            } else if (ctx != null) {
                try {
                    ctx.stopService(new Intent(ctx, ConnectionService.class));
                } catch (RuntimeException error) {
                    android.util.Log.w("paxri", "Stopping gateway service", error);
                }
            }
            return;
        }
        if (active != null) {
            active.applyState();
        } else if (shouldStart) {
            try {
                ctx.startForegroundService(new Intent(ctx, ConnectionService.class));
            } catch (IllegalStateException | SecurityException error) {
                synchronized (LOCK) {
                    serviceStartRequested = false;
                }
                android.util.Log.w("paxri", "Starting gateway foreground service", error);
            }
        }
    }

    @Override public void onCreate() {
        super.onCreate();
        notifications = getSystemService(NotificationManager.class);
        if (notifications != null) {
            notifications.deleteNotificationChannel(LEGACY_CHANNEL);
            NotificationChannel channel = new NotificationChannel(
                    CHANNEL, "Gateway service", NotificationManager.IMPORTANCE_MIN);
            channel.setShowBadge(false);
            channel.setSound(null, null);
            channel.enableVibration(false);
            notifications.createNotificationChannel(channel);
        }
        startForeground(
                NOTIFICATION_ID,
                buildNotification(),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE);
        boolean shouldKeepRunning;
        synchronized (LOCK) {
            appContext = getApplicationContext();
            instance = this;
            serviceStartRequested = false;
            shouldKeepRunning = gatewayRunning && backgroundService;
        }
        registerNetworkCallback();
        if (shouldKeepRunning) {
            applyState();
        } else {
            releaseLocks();
            stopSelf();
        }
    }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && ACTION_STOP.equals(intent.getAction())) {
            synchronized (LOCK) {
                gatewayRunning = false;
                serviceStartRequested = false;
            }
            notifyStopRequested();
            releaseLocks();
            stopSelf();
            return START_NOT_STICKY;
        }
        boolean shouldKeepRunning;
        synchronized (LOCK) {
            shouldKeepRunning = gatewayRunning && backgroundService;
        }
        if (shouldKeepRunning) {
            applyState();
        } else {
            releaseLocks();
            stopSelf();
        }
        return START_NOT_STICKY;
    }

    @Override public void onTaskRemoved(Intent rootIntent) {
        notifyTaskRemoved();
        boolean keepInBackground;
        synchronized (LOCK) {
            keepInBackground = gatewayRunning && backgroundService;
        }
        if (!keepInBackground) {
            releaseLocks();
            stopSelf();
        }
        super.onTaskRemoved(rootIntent);
    }

    @Override public void onDestroy() {
        synchronized (LOCK) {
            if (instance == this) {
                instance = null;
            }
            serviceStartRequested = false;
        }
        unregisterNetworkCallback();
        releaseLocks();
        super.onDestroy();
    }

    @Override public IBinder onBind(Intent intent) { return null; }

    private Notification buildNotification() {
        int connected;
        int paired;
        synchronized (LOCK) {
            connected = connectedCount;
            paired = pairedCount;
        }
        String text;
        if (connected == 1) {
            text = "Gateway running · 1 device connected";
        } else if (connected > 1) {
            text = "Gateway running · " + connected + " devices connected";
        } else if (paired > 0) {
            text = "Gateway running · Waiting for paired devices";
        } else {
            text = "Gateway running";
        }
        Intent open = getPackageManager().getLaunchIntentForPackage(getPackageName());
        PendingIntent content = PendingIntent.getActivity(this, 0, open,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Intent stopIntent = new Intent(this, ConnectionService.class).setAction(ACTION_STOP);
        PendingIntent stopPending = PendingIntent.getService(this, 1, stopIntent,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification.Action stopAction = new Notification.Action.Builder(null, "Stop", stopPending).build();
        return new Notification.Builder(this, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_notify_sync)
                .setContentTitle("paxri")
                .setContentText(text)
                .setContentIntent(content)
                .addAction(stopAction)
                .setOnlyAlertOnce(true)
                .setOngoing(true)
                .setShowWhen(false)
                .build();
    }

    private void applyState() {
        if (notifications != null) {
            try {
                notifications.notify(NOTIFICATION_ID, buildNotification());
            } catch (RuntimeException error) {
                android.util.Log.w("paxri", "Updating gateway notification", error);
            }
        }
        boolean keepAlive;
        synchronized (LOCK) {
            keepAlive = gatewayRunning && backgroundService && screenOffKeepAlive;
        }
        if (keepAlive) {
            acquireLocks();
        } else {
            releaseLocks();
        }
    }

    private void acquireLocks() {
        try {
            if (wakeLock == null) {
                PowerManager power = getSystemService(PowerManager.class);
                if (power != null) {
                    wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "paxri:ConnectionService");
                    wakeLock.setReferenceCounted(false);
                }
            }
            if (wakeLock != null && !wakeLock.isHeld()) {
                wakeLock.acquire();
            }
        } catch (RuntimeException error) {
            android.util.Log.w("paxri", "Acquiring CPU wake lock", error);
        }
        try {
            if (wifiLock == null) {
                WifiManager wifi = (WifiManager) getApplicationContext().getSystemService(Context.WIFI_SERVICE);
                if (wifi != null) {
                    wifiLock = wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "paxri:ConnectionService");
                    wifiLock.setReferenceCounted(false);
                }
            }
            if (wifiLock != null && !wifiLock.isHeld()) {
                wifiLock.acquire();
            }
        } catch (RuntimeException error) {
            android.util.Log.w("paxri", "Acquiring Wi-Fi lock", error);
        }
    }

    private void releaseLocks() {
        try {
            if (wakeLock != null && wakeLock.isHeld()) {
                wakeLock.release();
            }
        } catch (RuntimeException error) {
            android.util.Log.w("paxri", "Releasing CPU wake lock", error);
        }
        try {
            if (wifiLock != null && wifiLock.isHeld()) {
                wifiLock.release();
            }
        } catch (RuntimeException error) {
            android.util.Log.w("paxri", "Releasing Wi-Fi lock", error);
        }
    }

    private static String linkSignature(LinkProperties properties) {
        if (properties == null) return "";
        List<String> items = new ArrayList<>();
        String iface = properties.getInterfaceName();
        if (iface != null) items.add("if:" + iface);
        for (LinkAddress address : properties.getLinkAddresses()) {
            items.add(address.toString());
        }
        Collections.sort(items);
        StringBuilder out = new StringBuilder();
        for (String item : items) {
            out.append(item).append(';');
        }
        return out.toString();
    }

    private void notifyNetworkChanged() {
        try {
            networkChanged();
        } catch (UnsatisfiedLinkError ignored) {
            // Native library has not loaded yet.
        }
    }

    private void notifyStopRequested() {
        try {
            stopRequested();
        } catch (UnsatisfiedLinkError ignored) {
            // Native library has not loaded yet.
        }
    }

    private void notifyTaskRemoved() {
        try {
            taskRemoved();
        } catch (UnsatisfiedLinkError ignored) {
            // Native library has not loaded yet.
        }
    }

    private void registerNetworkCallback() {
        connectivity = getSystemService(ConnectivityManager.class);
        if (connectivity == null) return;
        networkCallback = new ConnectivityManager.NetworkCallback() {
            @Override public void onAvailable(Network network) {
                boolean changed;
                synchronized (ConnectionService.this) {
                    changed = activeNetwork != null && !activeNetwork.equals(network);
                    activeNetwork = network;
                    if (changed) activeLinkSignature = null;
                }
                if (changed) notifyNetworkChanged();
            }

            @Override public void onLinkPropertiesChanged(Network network, LinkProperties properties) {
                String sig = linkSignature(properties);
                boolean changed;
                synchronized (ConnectionService.this) {
                    if (activeNetwork != null && !activeNetwork.equals(network)) {
                        activeNetwork = network;
                        activeLinkSignature = sig;
                        changed = true;
                    } else if (activeLinkSignature == null) {
                        activeNetwork = network;
                        activeLinkSignature = sig;
                        changed = false;
                    } else {
                        changed = !activeLinkSignature.equals(sig);
                        activeLinkSignature = sig;
                    }
                }
                if (changed) notifyNetworkChanged();
            }

            @Override public void onLost(Network network) {
                boolean lostActive;
                synchronized (ConnectionService.this) {
                    lostActive = activeNetwork != null && activeNetwork.equals(network);
                    if (lostActive) {
                        activeNetwork = null;
                        activeLinkSignature = null;
                    }
                }
                if (lostActive) notifyNetworkChanged();
            }
        };
        try {
            connectivity.registerDefaultNetworkCallback(networkCallback);
        } catch (RuntimeException error) {
            networkCallback = null;
            android.util.Log.w("paxri", "Registering network callback", error);
        }
    }

    private void unregisterNetworkCallback() {
        if (connectivity != null && networkCallback != null) {
            try {
                connectivity.unregisterNetworkCallback(networkCallback);
            } catch (RuntimeException ignored) {
                // Already unregistered.
            }
        }
        networkCallback = null;
    }
}
