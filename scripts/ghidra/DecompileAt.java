// Ghidra headless post-script: decompile code at the given addresses, creating a function where
// the analysis left only a label (task callbacks reached through data pointers). Writes
// <outdir>/<addr>.c; the output is game-derived and must stay in gitignored re/.
// Args: <outdir> <hex address>...
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.app.cmd.function.CreateFunctionCmd;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import java.io.*;

public class DecompileAt extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File out = new File(args[0]);
        out.mkdirs();
        DecompInterface d = new DecompInterface();
        d.openProgram(currentProgram);
        for (int i = 1; i < args.length; i++) {
            Address a = toAddr(Long.parseLong(args[i].replace("0x", ""), 16));
            Function f = getFunctionAt(a);
            if (f == null) {
                disassemble(a);
                new CreateFunctionCmd(a).applyTo(currentProgram, monitor);
                f = getFunctionAt(a);
            }
            String name = String.format("%08x", a.getOffset());
            try (PrintWriter w = new PrintWriter(new File(out, name + ".c"), "UTF-8")) {
                if (f == null) {
                    w.println("// no function at " + name);
                    continue;
                }
                DecompileResults r = d.decompileFunction(f, 120, monitor);
                w.println("// " + f.getName() + " @ " + name);
                w.print(r.decompileCompleted() ? r.getDecompiledFunction().getC()
                        : "// decompile failed: " + r.getErrorMessage());
            }
        }
        d.dispose();
    }
}
