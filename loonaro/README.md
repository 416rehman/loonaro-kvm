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

JSON profiles contain Windows kernel symbols needed for introspection.

### Step 1: Get Kernel PDB Info

With the VM running:

```bash
# vmi-win-guid is built from kvm-vmi/libvmi/build/examples/
sudo kvm-vmi/libvmi/build/examples/vmi-win-guid name <vm> /tmp/introspector
```

Output:
```
Windows Kernel found @ 0x37200000
        PDB GUID: 910543a562cd3d9a19a2d8b087da182f1
        Kernel filename: ntkrla57.pdb
```

### Step 2: Download and Convert PDB

```bash
# install volatility3 if not already installed
pip install volatility3 pefile

# find pdbconv.py location
PDBCONV=$(python3 -c "import volatility3; print(volatility3.__path__[0])")/framework/symbols/windows/pdbconv.py

# convert PDB to JSON (downloads from Microsoft symbol server)
python3 $PDBCONV -p <kernel-filename> -g <pdb-guid> -o loonaro/templates/<name>.json

# Example:
# python3 $PDBCONV -p ntkrla57.pdb -g 910543a562cd3d9a19a2d8b087da182f1 -o loonaro/templates/win11.json
```

**Note**: JSON files are 5-15MB. Same file works for all VMs with identical Windows version.

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


