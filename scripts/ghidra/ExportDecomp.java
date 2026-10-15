// Ghidra headless post-script: write each function's decompilation to <outdir>/<addr>_<name>.c
// plus an index (functions.tsv). Output is game-derived and must stay in gitignored re/.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import java.io.*;

public class ExportDecomp extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File out = new File(args.length > 0 ? args[0] : "re/decomp");
        out.mkdirs();
        DecompInterface d = new DecompInterface();
        d.openProgram(currentProgram);
        try (PrintWriter idx = new PrintWriter(new File(out, "functions.tsv"), "UTF-8")) {
            idx.println("address\tname\tsize\tcallers\tcallees");
            for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
                if (monitor.isCancelled()) break;
                String addr = f.getEntryPoint().toString();
                idx.printf("%s\t%s\t%d\t%d\t%d%n", addr, f.getName(), f.getBody().getNumAddresses(),
                    f.getCallingFunctions(monitor).size(), f.getCalledFunctions(monitor).size());
                if (f.isThunk() || f.isExternal()) continue;
                DecompileResults r = d.decompileFunction(f, 60, monitor);
                String c = r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// decompile failed: " + r.getErrorMessage();
                try (PrintWriter w = new PrintWriter(new File(out, addr + "_" + f.getName().replaceAll("[^A-Za-z0-9_]", "_") + ".c"), "UTF-8")) {
                    w.println("// " + f.getName() + " @ " + addr);
                    w.print(c);
                }
            }
        }
        d.dispose();
    }
}
