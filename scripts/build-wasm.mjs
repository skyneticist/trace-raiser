import { spawn } from "node:child_process";
import { copyFile, mkdir, stat } from "node:fs/promises";
import { resolve } from "node:path";

const cargoHome = resolve("work/cargo-home");
const targetDirectory = resolve("work/cargo-target");

const builds = [
  {
    label: "geometry",
    manifest: "crates/pcb-core/Cargo.toml",
    artifact: "pcb_core.wasm",
    destination: "public/pcb_core.wasm",
  },
  {
    label: "AutoLayout",
    manifest: "crates/autolayout-wasm/Cargo.toml",
    artifact: "autolayout_wasm.wasm",
    destination: "public/autolayout_core.wasm",
  },
];

await mkdir(resolve("public"), { recursive: true });
for (const build of builds) {
  await new Promise((resolveRun, rejectRun) => {
    const cargo = spawn(
      "cargo",
      [
        "build",
        "--manifest-path",
        build.manifest,
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

  const source = resolve(targetDirectory, "wasm32-unknown-unknown/release", build.artifact);
  const destination = resolve(build.destination);
  await copyFile(source, destination);
  const output = await stat(destination);
  console.log(`Copied Rust ${build.label} engine (${Math.ceil(output.size / 1024)} KiB) to ${build.destination}`);
}
