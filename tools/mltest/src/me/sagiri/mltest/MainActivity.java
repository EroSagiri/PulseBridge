package me.sagiri.mltest;

import android.app.Activity;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothGatt;
import android.bluetooth.BluetoothGattCallback;
import android.bluetooth.BluetoothGattCharacteristic;
import android.bluetooth.BluetoothGattDescriptor;
import android.bluetooth.BluetoothGattService;
import android.bluetooth.BluetoothManager;
import android.content.Context;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;

import java.util.ArrayDeque;
import java.util.Arrays;
import java.util.Locale;
import java.util.UUID;

/**
 * Phase 0 go/no-go probe: can a second GATT client register a Garmin Multi-Link
 * service while the official Garmin Connect app stays connected?
 *
 * Modes (adb shell am start -e mode <m>):
 *   scan - discover and dump the whole GATT table. Read-only, writes nothing.
 *   reg  - additionally send Multi-Link handle-registration requests.
 */
public class MainActivity extends Activity {

    static final String TAG = "MLTEST";

    /** Garmin 128-bit UUID base: 6a4eXXXX-667b-11e3-949a-0800200c9a66 */
    static UUID garmin(int shortId) {
        return UUID.fromString(String.format(Locale.US,
                "6a4e%04x-667b-11e3-949a-0800200c9a66", shortId));
    }

    static final UUID CCCD = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb");

    /**
     * FR255 exposes three Multi-Link lanes: 2820/2810, 2821/2811 and 2840/2830.
     * (Not 2822/2812 as the Gadgetbridge notes suggest.) Garmin Connect was observed
     * notifying on 2811, so it owns lane 1; we probe lane 0 and never touch its stream.
     */
    static final UUID ML_WRITE = garmin(0x2820);
    static final UUID ML_READ = garmin(0x2810);

    /** Registration characteristic: takes single-byte capability queries. */
    static final UUID ML_REG = garmin(0x2803);

    /** Anything but 0x01, which is what Garmin Connect uses. "PB" then zeros. */
    static final byte[] CLIENT_UUID = {0x50, 0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00};

    static final int SVC_GFDI = 1;
    static final int SVC_REGISTRATION = 4;
    static final int SVC_REAL_TIME_HR = 6;

    private BluetoothGatt gatt;
    private String mode = "scan";
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final ArrayDeque<Runnable> queue = new ArrayDeque<Runnable>();

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        mode = getIntent().getStringExtra("mode");
        if (mode == null) mode = "scan";
        Log.i(TAG, "=== MLTEST start, mode=" + mode + " ===");

        BluetoothManager bm = (BluetoothManager) getSystemService(Context.BLUETOOTH_SERVICE);
        BluetoothAdapter adapter = bm.getAdapter();
        if (adapter == null || !adapter.isEnabled()) {
            Log.e(TAG, "bluetooth adapter unavailable or off");
            return;
        }

        BluetoothDevice target = null;
        for (BluetoothDevice d : adapter.getBondedDevices()) {
            String n = d.getName();
            Log.i(TAG, "bonded: " + d.getAddress() + "  " + n);
            if (n != null && n.toLowerCase(Locale.US).contains("forerunner")) target = d;
        }
        if (target == null) {
            Log.e(TAG, "no bonded device whose name contains forerunner");
            return;
        }

        Log.i(TAG, "connecting to " + target.getAddress() + " (" + target.getName() + ")");
        // autoConnect=false attaches to the already-open ACL link if one exists.
        gatt = target.connectGatt(this, false, callback, BluetoothDevice.TRANSPORT_LE);
    }

    private final BluetoothGattCallback callback = new BluetoothGattCallback() {

        @Override
        public void onConnectionStateChange(BluetoothGatt g, int status, int newState) {
            Log.i(TAG, "onConnectionStateChange status=" + status + " newState=" + newState
                    + (newState == BluetoothGatt.STATE_CONNECTED ? " (CONNECTED)" : ""));
            if (newState == BluetoothGatt.STATE_CONNECTED) {
                g.discoverServices();
            }
        }

        @Override
        public void onServicesDiscovered(BluetoothGatt g, int status) {
            Log.i(TAG, "onServicesDiscovered status=" + status);
            for (BluetoothGattService s : g.getServices()) {
                Log.i(TAG, "SERVICE " + s.getUuid());
                for (BluetoothGattCharacteristic c : s.getCharacteristics()) {
                    Log.i(TAG, "   CHAR " + c.getUuid() + "  props=0x"
                            + Integer.toHexString(c.getProperties()) + " " + props(c.getProperties()));
                }
            }
            if (!"reg".equals(mode)) {
                Log.i(TAG, "=== scan mode done, wrote nothing ===");
                return;
            }
            startRegistration(g);
        }

        @Override
        public void onCharacteristicChanged(BluetoothGatt g,
                                            BluetoothGattCharacteristic c, byte[] value) {
            Log.i(TAG, "NOTIFY " + shortName(c.getUuid()) + "  " + hex(value));
            decodeMl(value);
        }

        @Override
        public void onCharacteristicRead(BluetoothGatt g, BluetoothGattCharacteristic c,
                                         byte[] value, int status) {
            Log.i(TAG, "READ " + shortName(c.getUuid()) + " status=" + status + "  " + hex(value));
            if (ML_REG.equals(c.getUuid())) decodeSupported(value);
            next();
        }

        @Override
        public void onCharacteristicWrite(BluetoothGatt g,
                                          BluetoothGattCharacteristic c, int status) {
            Log.i(TAG, "onCharacteristicWrite " + shortName(c.getUuid()) + " status=" + status);
            next();
        }

        @Override
        public void onDescriptorWrite(BluetoothGatt g, BluetoothGattDescriptor d, int status) {
            Log.i(TAG, "onDescriptorWrite " + shortName(d.getCharacteristic().getUuid())
                    + " status=" + status);
            next();
        }
    };

    private void startRegistration(final BluetoothGatt g) {
        final BluetoothGattCharacteristic readChar = find(g, ML_READ);
        final BluetoothGattCharacteristic writeChar = find(g, ML_WRITE);
        final BluetoothGattCharacteristic regChar = find(g, ML_REG);
        if (readChar == null || writeChar == null) {
            Log.e(TAG, "lane 0 not present: read=" + readChar + " write=" + writeChar);
            return;
        }

        // Capability queries on the registration characteristic first: these claim nothing.
        if (regChar != null) {
            queue.add(readStep(g, regChar));
            queue.add(regQuery(g, regChar, 0x00, "SUPPORTED_PROTOCOLS"));
            queue.add(readStep(g, regChar));
            queue.add(regQuery(g, regChar, 0x02, "MULTI_LINK_VERSION"));
            queue.add(readStep(g, regChar));
            queue.add(regQuery(g, regChar, 0x03, "PRODUCT_NUMBER"));
            queue.add(readStep(g, regChar));
        }

        queue.add(new Runnable() {
            public void run() {
                Log.i(TAG, "-> enabling notifications on " + shortName(ML_READ));
                g.setCharacteristicNotification(readChar, true);
                BluetoothGattDescriptor cccd = readChar.getDescriptor(CCCD);
                if (cccd == null) {
                    Log.e(TAG, "no CCCD on " + shortName(ML_READ));
                    next();
                    return;
                }
                g.writeDescriptor(cccd, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE);
            }
        });
        queue.add(regStep(g, writeChar, SVC_REGISTRATION));
        queue.add(regStep(g, writeChar, SVC_REAL_TIME_HR));
        queue.add(regStep(g, writeChar, SVC_GFDI));
        queue.add(new Runnable() {
            public void run() {
                Log.i(TAG, "=== reg sequence done, watching notifications ===");
            }
        });

        next();
    }

    private Runnable readStep(final BluetoothGatt g, final BluetoothGattCharacteristic c) {
        return new Runnable() {
            public void run() {
                Log.i(TAG, "-> read " + shortName(c.getUuid()));
                if (!g.readCharacteristic(c)) {
                    Log.e(TAG, "   readCharacteristic returned false");
                    next();
                }
            }
        };
    }

    private Runnable regQuery(final BluetoothGatt g, final BluetoothGattCharacteristic c,
                              final int query, final String name) {
        return new Runnable() {
            public void run() {
                Log.i(TAG, "-> query " + name + " (0x" + Integer.toHexString(query) + ") on "
                        + shortName(c.getUuid()));
                int rc = g.writeCharacteristic(c, new byte[]{(byte) query},
                        BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT);
                Log.i(TAG, "   writeCharacteristic rc=" + rc);
                if (rc != BluetoothGatt.GATT_SUCCESS) next();
            }
        };
    }

    /**
     * Reply to SUPPORTED_PROTOCOLS: byte 0 echoes the query type, the rest is a
     * little-endian bitmap of service ids (bit n of byte i means service i*8+n).
     */
    private static void decodeSupported(byte[] v) {
        if (v.length < 2 || v[0] != 0x00) return;
        StringBuilder sb = new StringBuilder("   >>> supported services:");
        for (int i = 1; i < v.length; i++) {
            for (int bit = 0; bit < 8; bit++) {
                if ((v[i] & (1 << bit)) != 0) {
                    int id = (i - 1) * 8 + bit;
                    sb.append(' ').append(id).append('(').append(serviceName(id)).append(')');
                }
            }
        }
        Log.i(TAG, sb.toString());
    }

    private Runnable regStep(final BluetoothGatt g,
                             final BluetoothGattCharacteristic writeChar, final int service) {
        return new Runnable() {
            public void run() {
                register(g, writeChar, service);
            }
        };
    }

    private void register(BluetoothGatt g, BluetoothGattCharacteristic writeChar, int service) {
        // 0x00 0x00 | client_uuid[8] | service_id[2, LE] | 0x00 (ML, not reliable)
        byte[] frame = new byte[13];
        frame[0] = 0x00;
        frame[1] = 0x00;
        System.arraycopy(CLIENT_UUID, 0, frame, 2, 8);
        frame[10] = (byte) (service & 0xff);
        frame[11] = (byte) ((service >> 8) & 0xff);
        frame[12] = 0x00;

        Log.i(TAG, "-> REGISTER service=" + service + " (" + serviceName(service)
                + ") frame=" + hex(frame));
        int rc = g.writeCharacteristic(writeChar, frame,
                BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT);
        Log.i(TAG, "   writeCharacteristic rc=" + rc);
        if (rc != BluetoothGatt.GATT_SUCCESS) next();
    }

    private void next() {
        Runnable r = queue.poll();
        if (r != null) handler.postDelayed(r, 1500);
    }

    private static void decodeMl(byte[] v) {
        if (v.length >= 13 && v[0] == 0x00 && v[1] == 0x01) {
            int service = (v[10] & 0xff) | ((v[11] & 0xff) << 8);
            int status = v[12] & 0xff;
            String s;
            switch (status) {
                case 0x00: s = "SUCCESS"; break;
                case 0x01: s = "INVALID_SERVICE_ID"; break;
                case 0x02: s = "PENDING_AUTH"; break;
                case 0x03: s = "ALREADY_IN_USE"; break;
                case 0x04: s = "REJECTED"; break;
                default: s = "UNKNOWN(0x" + Integer.toHexString(status) + ")";
            }
            StringBuilder sb = new StringBuilder();
            sb.append("   >>> REG RESPONSE service=").append(service)
                    .append(" (").append(serviceName(service)).append(")")
                    .append(" status=").append(s);
            if (v.length > 13) sb.append(" handle=0x").append(Integer.toHexString(v[13] & 0xff));
            if (v.length > 14) sb.append(" rest=").append(hex(Arrays.copyOfRange(v, 14, v.length)));
            Log.i(TAG, sb.toString());
        }
    }

    private static String serviceName(int id) {
        switch (id) {
            case 1: return "GFDI";
            case 4: return "REGISTRATION";
            case 6: return "REAL_TIME_HR";
            case 12: return "REAL_TIME_HRV";
            case 13: return "REAL_TIME_STRESS";
            case 19: return "REAL_TIME_SPO2";
            case 20: return "REAL_TIME_BODY_BATTERY";
            default: return "svc" + id;
        }
    }

    private static BluetoothGattCharacteristic find(BluetoothGatt g, UUID uuid) {
        for (BluetoothGattService s : g.getServices()) {
            BluetoothGattCharacteristic c = s.getCharacteristic(uuid);
            if (c != null) return c;
        }
        return null;
    }

    private static String shortName(UUID u) {
        String s = u.toString();
        return s.startsWith("6a4e") ? s.substring(4, 8) : s;
    }

    private static String props(int p) {
        StringBuilder sb = new StringBuilder();
        if ((p & BluetoothGattCharacteristic.PROPERTY_READ) != 0) sb.append("READ ");
        if ((p & BluetoothGattCharacteristic.PROPERTY_WRITE) != 0) sb.append("WRITE ");
        if ((p & BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE) != 0) sb.append("WRITE_NR ");
        if ((p & BluetoothGattCharacteristic.PROPERTY_NOTIFY) != 0) sb.append("NOTIFY ");
        if ((p & BluetoothGattCharacteristic.PROPERTY_INDICATE) != 0) sb.append("INDICATE ");
        return sb.toString().trim();
    }

    private static String hex(byte[] b) {
        StringBuilder sb = new StringBuilder();
        for (byte x : b) sb.append(String.format(Locale.US, "%02x ", x));
        return sb.toString().trim();
    }
}
