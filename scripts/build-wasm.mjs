import { spawn } from "node:child_process";
import { copyFile, mkdir, stat } from "node:fs/promises";
import { resolve } from "node:path";

const cargoHome = resolve("work/cargo-home");
const targetDirectory = resolve("work/cargo-target");

await new Promise((resolveRun, rejectRun) => {
  const cargo = spawn(
    "cargo",
    [
      "build",
      "--manifest-path",
      "crates/pcb-core/Cargo.toml",
      "--target",
      "wasm32-unknown-unknown",
      "--release",
    ],
    {
      stdio: "inherit",
      env: {
        ...process.env,
        CARGO_HOME: cargoHome,
        CARGO_TARGET_DIR: targetDirectory,
      },
    },
  );

  cargo.on("error", rejectRun);
  cargo.on("exit", (code) => {
    if (code === 0) resolveRun();
    else rejectRun(new Error(`cargo build exited with code ${code ?? "unknown"}`));
  });
});

const source = resolve(targetDirectory, "wasm32-unknown-unknown/release/pcb_core.wasm");
const destination = resolve("public/pcb_core.wasm");

await mkdir(resolve("public"), { recursive: true });
await copyFile(source, destination);

const output = await stat(destination);
console.log(`Copied Rust geometry engine (${Math.ceil(output.size / 1024)} KiB) to public/pcb_core.wasm`);
