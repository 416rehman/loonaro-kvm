# Loonaro: Stealthy KVM-VMI Sandbox

Polymorphic VM orchestration with KVM-VMI introspection support.

## Quick Start

```bash
# list templates
./scripts/setup.sh --list

# create vm
./scripts/setup.sh win11 my-sandbox ~/Downloads/win11.iso

# start vm
./scripts/start.sh --gui my-sandbox

# teardown
./scripts/teardown.sh my-sandbox
```

## Directory Structure

```
loonaro/
├── scripts/        # setup.sh, start.sh, teardown.sh
├── templates/      # VM definitions (.xml + .json pairs)
├── config/         # network configs
└── vms/            # runtime artifacts (disks, logs)
```

## Templates

Each template requires **two files** in `templates/`:

| File | Purpose |
|------|---------|
| `<name>.xml` | Libvirt VM definition |
| `<name>.json` | Volatility IST (kernel symbols for introspection) |

### Adding a New Template

1. **Create XML**: Copy `win11.xml` → `<name>.xml`, edit as needed
2. **Create JSON Profile**: See [Creating JSON Profiles](#creating-json-profiles)
3. **Run**: `./scripts/setup.sh <name> my-vm /path/to/iso`

---

### Building LibVMI

```bash
cd kvm-vmi/libvmi
mkdir -p build && cd build

# Release build
cmake .. -DCMAKE_INSTALL_PREFIX=/usr/local \
  -DENABLE_KVM=ON -DENABLE_XEN=OFF -DENABLE_BAREFLANK=OFF

# OR Debug build (verbose output)
cmake .. -DCMAKE_INSTALL_PREFIX=/usr/local \
  -DENABLE_KVM=ON -DENABLE_XEN=OFF -DENABLE_BAREFLANK=OFF \
  -DVMI_DEBUG=0xFFFF -DENV_DEBUG=ON

make -j$(nproc)
sudo make install && sudo ldconfig
```

---

## Creating JSON Profiles

JSON profiles contain Windows kernel symbols needed for introspection. **The profile must match the exact kernel version running in the VM** - if Windows updates, you need to regenerate it.

### Step 1: Get Kernel PDB Info

With the VM running and idle:

```bash
# vmi-win-guid outputs kernel GUID and filename
sudo kvm-vmi/libvmi/build/examples/vmi-win-guid name <vm> /tmp/introspector
```

Example output:
```
Windows Kernel found @ 0x100200000
        Version: 64-bit Windows 10
        PDB GUID: 0762cf42ef7f3e8116ef7329adaa09a31
        Kernel filename: ntkrnlmp.pdb
```

### Step 2: Download PDB from Microsoft Symbol Server

```bash
# format: uppercase GUID
wget "https://msdl.microsoft.com/download/symbols/<filename>/<GUID>/<filename>" -O /tmp/<filename>

# example for Windows 11 25H2:
wget "https://msdl.microsoft.com/download/symbols/ntkrnlmp.pdb/0762CF42EF7F3E8116EF7329ADAA09A31/ntkrnlmp.pdb" -O /tmp/ntkrnlmp.pdb
```

### Step 3: Convert PDB to JSON

```bash
# install volatility3 if not already installed
pip install volatility3 pefile

# find pdbconv.py location
PDBCONV=$(python3 -c "import volatility3; print(volatility3.__path__[0])")/framework/symbols/windows/pdbconv.py

python3 $PDBCONV -f /tmp/ntkrnlmp.pdb -o loonaro/vms/<vm>.json
```

### Troubleshooting: "All KdDebuggerDataBlock search methods failed"

This error means the JSON profile doesn't match the running kernel. Common causes:

1. **Windows updated** - Regenerate the profile with the new GUID
2. **Wrong GUID format** - Remove trailing `1`, use uppercase
3. **VM not fully booted** - Wait for desktop to load before running vmi-win-guid

Verify GUID match:
```bash
# check running kernel GUID
sudo kvm-vmi/libvmi/build/examples/vmi-win-guid name <vm> /tmp/introspector | grep "PDB GUID"

# check JSON profile GUID
python3 -c "import json; print(json.load(open('loonaro/vms/<vm>.json'))['metadata']['windows']['pdb']['GUID'])"
```

**These GUIDs must match** (ignoring case and trailing age digit).

---


## Scripts

| Script | Description |
|--------|-------------|
| `setup.sh <template> [name] [iso]` | Creates VM with random UUID/MAC/Serial, copies JSON profile |
| `start.sh [--gui] <name>` | Starts VM, optionally opens viewer |
| `teardown.sh <name>` | Destroys VM, removes disk, cleans up JSON |

---

## Running Introspection Apps

```bash
# Process listing (requires JSON profile)
sudo kvm-vmi/libvmi/build/examples/vmi-process-list \
  -n <vm> -s /tmp/introspector -j /var/lib/libvmi/<vm>.json

# With debug output
sudo LIBVMI_DEBUG=1 kvm-vmi/libvmi/build/examples/vmi-process-list \
  -n <vm> -s /tmp/introspector -j /var/lib/libvmi/<vm>.json
```

### Custom Apps

Build custom apps in `apps/`:

```bash
cd apps && make
sudo ./vmi-process-list -n <vm> -s /tmp/introspector -j /var/lib/libvmi/<vm>.json
```

---

## Prerequisites

```bash
sudo apt install -y qemu-kvm libvirt-daemon-system libvirt-clients \
  bridge-utils virt-manager apparmor-utils ovmf swtpm swtpm-tools
```


