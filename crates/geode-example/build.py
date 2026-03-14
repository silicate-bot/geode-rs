import os
import shutil
import subprocess
import sys

MOD_ID = "test.example_mod"
EXAMPLE_DIR = os.path.dirname(os.path.realpath(__file__))
PROJECT_ROOT = os.path.abspath(os.path.join(EXAMPLE_DIR, "../.."))


def build(target):
    binaries = []

    platforms = {
        "win": build_windows,
        "mac": build_mac,
        "linux": build_linux,
        "android": build_android,
    }

    if target == "all":
        for platform in platforms.values():
            binaries.extend(platform())
    elif target in platforms:
        binaries.extend(platforms[target]())

    package_binaries(binaries)


def build_windows():
    result = subprocess.run(
        ["cargo", "build", "--release", "-p", "geode-example"], cwd=PROJECT_ROOT
    )
    if result.returncode != 0:
        return []

    win_dll = os.path.join(PROJECT_ROOT, "target", "release", "geode_example.dll")
    if not os.path.exists(win_dll):
        return []

    win_dll_named = os.path.join(PROJECT_ROOT, "target", "release", f"{MOD_ID}.dll")
    shutil.copy(win_dll, win_dll_named)
    return [win_dll_named]


def build_mac():
    binaries = []
    mac_arm = build_mac_arm()
    mac_x64 = build_mac_x64()
    mac_universal = combine_mac_binaries(mac_arm, mac_x64)

    if mac_universal:
        binaries.append(mac_universal)
    return binaries


def build_mac_arm():
    result = subprocess.run(
        [
            "cargo",
            "zigbuild",
            "--target",
            "aarch64-apple-darwin",
            "--release",
            "-p",
            "geode-example",
        ],
        cwd=PROJECT_ROOT,
    )
    if result.returncode != 0:
        return None

    mac_arm = os.path.join(
        PROJECT_ROOT,
        "target",
        "aarch64-apple-darwin",
        "release",
        "libgeode_example.dylib",
    )
    return mac_arm if os.path.exists(mac_arm) else None


def build_mac_x64():
    result = subprocess.run(
        [
            "cargo",
            "zigbuild",
            "--target",
            "x86_64-apple-darwin",
            "--release",
            "-p",
            "geode-example",
        ],
        cwd=PROJECT_ROOT,
    )
    if result.returncode != 0:
        return None

    mac_x64 = os.path.join(
        PROJECT_ROOT,
        "target",
        "x86_64-apple-darwin",
        "release",
        "libgeode_example.dylib",
    )
    return mac_x64 if os.path.exists(mac_x64) else None


def combine_mac_binaries(mac_arm, mac_x64):
    if not mac_arm or not mac_x64:
        return None

    mac_universal = os.path.join(PROJECT_ROOT, "target", f"{MOD_ID}.dylib")
    result = subprocess.run(
        ["cargo", "run", "-p", "mac-universal", "--", mac_arm, mac_x64, mac_universal],
        cwd=PROJECT_ROOT,
    )
    return mac_universal if result.returncode == 0 else None


def build_linux():
    result = subprocess.run(
        ["cargo", "build", "--release", "-p", "geode-example"], cwd=PROJECT_ROOT
    )
    if result.returncode != 0:
        return []

    linux_bin = os.path.join(PROJECT_ROOT, "target", "release", "geode_example")
    if not os.path.exists(linux_bin):
        return []

    linux_bin_named = os.path.join(PROJECT_ROOT, "target", "release", f"{MOD_ID}.linux")
    shutil.copy(linux_bin, linux_bin_named)
    return [linux_bin_named]


def build_android():
    binaries = []
    android_arm64 = build_android_arm64()
    android_arm32 = build_android_arm32()

    if android_arm64:
        binaries.append(android_arm64)
    if android_arm32:
        binaries.append(android_arm32)

    return binaries


def build_android_arm64():
    result = subprocess.run(
        [
            "cargo",
            "ndk",
            "--target",
            "aarch64-linux-android",
            "--platform",
            "21",
            "--",
            "build",
            "--release",
            "-p",
            "geode-example",
        ],
        cwd=PROJECT_ROOT,
    )
    if result.returncode != 0:
        return None

    so64 = os.path.join(
        PROJECT_ROOT,
        "target",
        "aarch64-linux-android",
        "release",
        "libgeode_example.so",
    )
    if os.path.exists(so64):
        so64_named = os.path.join(
            PROJECT_ROOT,
            "target",
            "aarch64-linux-android",
            "release",
            f"{MOD_ID}.android64.so",
        )
        shutil.copy(so64, so64_named)
        return so64_named
    return None


def build_android_arm32():
    result = subprocess.run(
        [
            "cargo",
            "ndk",
            "--target",
            "armv7-linux-androideabi",
            "--platform",
            "21",
            "--",
            "build",
            "--release",
            "-p",
            "geode-example",
        ],
        cwd=PROJECT_ROOT,
    )
    if result.returncode != 0:
        return None

    so32 = os.path.join(
        PROJECT_ROOT,
        "target",
        "armv7-linux-androideabi",
        "release",
        "libgeode_example.so",
    )
    if os.path.exists(so32):
        so32_named = os.path.join(
            PROJECT_ROOT,
            "target",
            "armv7-linux-androideabi",
            "release",
            f"{MOD_ID}.android32.so",
        )
        shutil.copy(so32, so32_named)
        return so32_named
    return None


def package_binaries(binaries):
    if not binaries:
        print("No binaries were built successfully. Aborting.")
        sys.exit(1)

    output = os.path.join(EXAMPLE_DIR, f"{MOD_ID}.geode")

    args = ["geode", "package", "new", EXAMPLE_DIR]
    for binary in binaries:
        args += ["--binary", binary]
    args += ["--output", output]

    result = subprocess.run(args, cwd=PROJECT_ROOT)
    if result.returncode != 0:
        print("Packaging failed.")
        sys.exit(result.returncode)

    print(f"Created {output}")
    print(
        "To install on Android, copy to:\n/storage/emulated/0/Android/media/com.geode.launcher/game/geode/mods/"
    )


if __name__ == "__main__":
    target = sys.argv[1] if len(sys.argv) > 1 else "all"
    build(target)
