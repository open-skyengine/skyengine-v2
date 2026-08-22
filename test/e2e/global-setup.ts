import { execFileSync } from "node:child_process";

export default function setup(): void {
  if (process.env.SKYENGINE_BIN) return;

  execFileSync("cargo", ["build", "--release", "-p", "skyengine"], {
    stdio: "inherit"
  });
}
