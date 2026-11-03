// unluac (unluac_2015_06_13.jar) on many Lua chunks in one JVM: `java -cp unluac.jar;tools UnluacBatch LIST`. LIST has
// one "in<TAB>out" line per chunk; each out gets the bytes `java -jar unluac.jar in` prints, and out.rc its exit code.
// tools/unluac_batch.py runs it. The build's Java runtime has no compiler, so UnluacBatch.class is committed beside this
// file; after changing it: javac --release 21 -cp unluac.jar -d tools tools/UnluacBatch.java
//
// unluac.Main.main ends in System.exit, so this does what it does between reading the file and exiting:
// Main.file_to_function (private), then Decompiler.decompile() and print(), whose Output prints to System.out.
// Two statics carry over between files in one JVM (VariableFinder.lc names a stripped chunk's locals, Decompiler.backup
// is left set if a decompile throws), so both are reset before each file: no output depends on the file order.
import java.io.ByteArrayOutputStream;
import java.io.PrintStream;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import unluac.Configuration;
import unluac.decompile.Decompiler;
import unluac.parse.LFunction;

public class UnluacBatch {
    public static void main(String[] args) throws Exception {
        PrintStream real = System.out;
        Method load = unluac.Main.class.getDeclaredMethod("file_to_function", String.class, Configuration.class);
        load.setAccessible(true);
        Field lc = Class.forName("unluac.decompile.VariableFinder").getDeclaredField("lc");
        lc.setAccessible(true);
        Field backup = Decompiler.class.getDeclaredField("backup");
        backup.setAccessible(true);
        for (String line : Files.readAllLines(Path.of(args[0]))) {
            String[] io = line.split("\t");
            ByteArrayOutputStream buffer = new ByteArrayOutputStream();
            System.setOut(new PrintStream(buffer, true, real.charset()));  // the CLI's stdout charset
            int code = 0;
            try {
                lc.setInt(null, 0);
                backup.set(null, null);
                LFunction main = (LFunction) load.invoke(null, io[0], new Configuration());
                Decompiler decompiler = new Decompiler(main);
                decompiler.decompile();
                decompiler.print();
            } catch (Throwable error) {  // the CLI exits 1 too, keeping what it printed so far
                code = 1;
            }
            System.out.flush();
            System.setOut(real);
            Files.write(Path.of(io[1]), buffer.toByteArray());
            Files.writeString(Path.of(io[1] + ".rc"), Integer.toString(code));
        }
    }
}
