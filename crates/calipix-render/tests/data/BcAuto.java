// Reference output for the B&C "Auto" button (ij.plugin.frame.ContrastAdjuster.autoAdjust).
//
// The button cannot be pressed from a batch macro, so `cargo xtask gen` cannot produce this
// reference. This program calls the method itself, by reflection, on a ContrastAdjuster whose
// window is never built, twelve times in a row per image, and prints the display range after
// each press. Regenerate bc_auto.csv (ImageJ 1.54p, the jar pinned in xtask) with:
//
//   java -cp path/to/ij-1.54p.jar BcAuto.java > bc_auto.csv
//
// Without a window the method's final scroll bar update throws a NullPointerException, after
// the display range has been set; that is caught here. When autoAdjust falls back to reset(),
// the same exception skips reset()'s last statement, `autoThreshold = 0`, which is therefore
// done here.
//
// Images are the 64x48 patterns of fixtures/imagej/gen/33_histogram.ijm (u8, u16, u16skew, f32,
// f32s, rgb). Output columns: image, press, autoThreshold after the press, "auto" or "reset",
// display range minimum, display range maximum.
import ij.ImagePlus;
import ij.plugin.frame.ContrastAdjuster;
import ij.process.ByteProcessor;
import ij.process.ColorProcessor;
import ij.process.FloatProcessor;
import ij.process.ImageProcessor;
import ij.process.ShortProcessor;
import java.lang.reflect.Field;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;

public class BcAuto {
    static double value(String kind, double x, double y) {
        switch (kind) {
            case "u8": return (x*x + 3*y*y + x*y) % 256;
            case "u16": return (x*257 + y*y*31 + 1000*((Math.floor(x/8)+Math.floor(y/8))%2)) % 65536;
            case "u16skew": return 100 + Math.floor(Math.pow((x*y)%97, 2)/3);
            default: return (x-31.5)*0.25 + (y*y)/64.0 - 3.5*((x+y)%5);
        }
    }

    static Field field(String name) throws Exception {
        Field f = ContrastAdjuster.class.getDeclaredField(name);
        f.setAccessible(true);
        return f;
    }

    public static void main(String[] args) throws Exception {
        String[] kinds = {"u8", "u16", "u16skew", "f32", "f32s", "rgb"};
        for (String kind : kinds) {
            ImageProcessor ip;
            if (kind.equals("rgb")) {
                ip = new ColorProcessor(64, 48);
                for (int y=0; y<48; y++) for (int x=0; x<64; x++)
                    ip.set(x, y, (((x*4)&255)<<16) | (((y*5)&255)<<8) | (((x+y)*3)&255));
                ip.snapshot(); // as ContrastAdjuster.setupNewImage does
            } else {
                ip = kind.equals("u8") ? new ByteProcessor(64, 48)
                    : kind.startsWith("u16") ? new ShortProcessor(64, 48) : new FloatProcessor(64, 48);
                for (int y=0; y<48; y++) for (int x=0; x<64; x++)
                    ip.setf(x, y, (float)value(kind, x, y));
            }
            if (kind.equals("f32s")) {
                ip.setf(3, 0, Float.NaN); ip.setf(5, 7, Float.NaN);
                ip.setf(20, 1, Float.POSITIVE_INFINITY); ip.setf(10, 2, Float.NEGATIVE_INFINITY);
            }
            ip.resetMinAndMax();
            ImagePlus imp = new ImagePlus(kind, ip);
            ContrastAdjuster adjuster = new ContrastAdjuster();
            boolean eightBit = kind.equals("u8") || kind.equals("rgb");
            field("defaultMin").setDouble(adjuster, eightBit ? 0 : ip.getMin());
            field("defaultMax").setDouble(adjuster, eightBit ? 255 : ip.getMax());
            field("RGBImage").setBoolean(adjuster, kind.equals("rgb"));
            Field autoThreshold = field("autoThreshold");
            Method autoAdjust = ContrastAdjuster.class.getDeclaredMethod("autoAdjust", ImagePlus.class, ImageProcessor.class);
            autoAdjust.setAccessible(true);
            for (int press=1; press<=12; press++) {
                boolean reset = false;
                try {
                    autoAdjust.invoke(adjuster, imp, imp.getProcessor());
                } catch (InvocationTargetException e) {
                    if (!(e.getCause() instanceof NullPointerException)) throw e;
                    for (StackTraceElement frame : e.getCause().getStackTrace())
                        if (frame.getMethodName().equals("reset")) reset = true;
                }
                int threshold = autoThreshold.getInt(adjuster);
                if (reset) autoThreshold.setInt(adjuster, 0);
                System.out.print(kind + "," + press + "," + threshold + "," + (reset ? "reset" : "auto") + ","
                    + imp.getDisplayRangeMin() + "," + imp.getDisplayRangeMax() + "\n");
            }
        }
        System.exit(0);
    }
}
