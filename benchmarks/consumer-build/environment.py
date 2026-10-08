"""Record toolchain, hardware, cache and generator identity for measurements."""
import datetime
import hashlib
import os
import platform
import subprocess
import prepare


def capture(options):
    rust = subprocess.check_output(["rustc", "-Vv"], text=True).strip()
    assert not os.environ.get("RUSTC_WRAPPER"), "disable compiler wrappers for matched cold builds"
    assert not os.environ.get("RUSTC_WORKSPACE_WRAPPER"), "disable workspace compiler wrappers for matched cold builds"
    hardware = {}
    if platform.system() == "Darwin":
        hardware = {"cpu": subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip(),
                    "memory_bytes": int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True))}
    return {"date_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "hardware": hardware, "platform": platform.platform(), "machine": platform.machine(), "cpu_count": os.cpu_count(),
            "rustc": rust, "cargo": subprocess.check_output([options.cargo, "-V"], text=True).strip(),
            "jobs": options.jobs, "lock_sha256": prepare.lock_hash(),
            "generator_binary": {"path": str(options.based), "bytes": options.based.stat().st_size,
                                 "version": subprocess.check_output([str(options.based), "--version"], text=True).strip(),
                                 "profile": "workspace release, thin LTO; installation/build outside consumer timings"},
            "profile": "consumer dev default; no workspace profile inheritance",
            "cache": "new target directory per variant/trial; registry sources pre-fetched, offline; no sccache",
            "generator_sha256": hashlib.sha256(options.based.read_bytes()).hexdigest(),
            "generator_workspace_lock_sha256": hashlib.sha256((prepare.ROOT / "Cargo.lock").read_bytes()).hexdigest(),
            "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=prepare.ROOT, text=True).strip()}


