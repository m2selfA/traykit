"""Verify the built PE contains the exact icon payloads and manifest from assets."""
import hashlib
import json
import pathlib
import struct
import tomllib
import pefile

root = pathlib.Path(__file__).resolve().parents[1]
binary = root / 'target/release/traykit.exe'
pe = pefile.PE(str(binary))
assert pe.FILE_HEADER.Machine == 0x8664
assert pe.OPTIONAL_HEADER.Subsystem == 2, 'Expected Windows GUI subsystem'
package_version = tomllib.loads((root / 'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
version_strings = {}
for blocks in pe.FileInfo or []:
    for block in blocks:
        for table in getattr(block, 'StringTable', []):
            version_strings.update({key.decode(): value.decode() for key, value in table.entries.items()})
file_version = version_strings.get('FileVersion')
product_version = version_strings.get('ProductVersion')
fixed = pe.VS_FIXEDFILEINFO[0]
fixed_version = '.'.join(map(str, [fixed.FileVersionMS >> 16, fixed.FileVersionMS & 0xffff,
                                   fixed.FileVersionLS >> 16, fixed.FileVersionLS & 0xffff]))
assert file_version == package_version, f'FileVersion {file_version!r} != Cargo version {package_version!r}'
assert product_version == package_version, f'ProductVersion {product_version!r} != Cargo version {package_version!r}'
assert fixed_version == f'{package_version}.0', f'Fixed file version {fixed_version!r} does not match Cargo version'
resources = {}
for entry in pe.DIRECTORY_ENTRY_RESOURCE.entries:
    resources[entry.id] = [
        pe.get_data(lang.data.struct.OffsetToData, lang.data.struct.Size)
        for item in entry.directory.entries for lang in item.directory.entries
    ]
ico = (root / 'assets/traykit.ico').read_bytes()
reserved, kind, count = struct.unpack_from('<HHH', ico)
assert (reserved, kind) == (0, 1)
asset_payloads = []
sizes = []
for index in range(count):
    width, height, colors, pad, planes, bits, length, offset = struct.unpack_from('<BBBBHHII', ico, 6+index*16)
    asset_payloads.append(hashlib.sha256(ico[offset:offset+length]).hexdigest())
    sizes.append([width or 256, height or 256])
embedded_payloads = [hashlib.sha256(data).hexdigest() for data in resources[3]]
assert sorted(asset_payloads) == sorted(embedded_payloads), 'Embedded icon differs from Agnes-derived ICO'
assert 14 in resources and len(resources[14]) == 1, 'Missing group icon'
manifest = resources[24][0].decode('utf-8')
assert 'asInvoker' in manifest and 'PerMonitorV2' in manifest
report = {
    'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
    'binary_bytes': binary.stat().st_size, 'architecture': 'x64', 'subsystem': 'Windows GUI',
    'icon_sizes': sizes, 'icon_payloads_match': True, 'manifest_as_invoker_per_monitor_v2': True,
    'package_version': package_version, 'file_version': file_version,
    'product_version': product_version, 'fixed_file_version': fixed_version,
    'source_hashes': {path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
                      for path in sorted(root.glob('src/**/*.rs')) + [root/'Cargo.toml', root/'Cargo.lock',
                          root/'build.rs', root/'assets/traykit.rc', root/'assets/traykit.ico',
                          root/'assets/app.manifest', root/'scripts/verify-resources.py',
                          root/'scripts/runtime-smoke.ps1', root/'scripts/NativeProbe.cs']}
}
out = root / 'target/verification/resources.json'
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8')
print(json.dumps({key: value for key, value in report.items() if key != 'source_hashes'}, indent=2))
