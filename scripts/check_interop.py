#!/usr/bin/env python3
"""Generate small synthetic masters and verify them with independent installed tools.

No proprietary tools or media are copied into the repository. Conversion uses the input sample
rate explicitly, so a 96 kHz acceptance check cannot silently pass by resampling to 48 kHz.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile


def run(args):
    return subprocess.run([str(v) for v in args], capture_output=True, text=True, timeout=90)


def wave(path):
    data = path.read_bytes()
    assert data[:4] in (b'RIFF', b'RF64', b'BW64') and data[8:12] == b'WAVE'
    data_size = None
    pos = 12
    chunks = {}
    while pos + 8 <= len(data):
        tag, length = struct.unpack_from('<4sI', data, pos)
        if length == 0xffffffff:
            assert tag == b'data' and data_size is not None
            length = data_size
        body = data[pos + 8:pos + 8 + length]
        assert len(body) == length
        chunks[tag] = body
        if tag == b'ds64':
            data_size = struct.unpack_from('<Q', body, 8)[0]
        pos += 8 + length + (length & 1)
    assert pos == len(data)
    channels, rate = struct.unpack_from('<HI', chunks[b'fmt '], 2)
    return channels, rate, chunks[b'data']


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--conversion-tool', required=True, type=Path)
    parser.add_argument('--atmos-info', type=Path)
    parser.add_argument('--mradm', type=Path)
    parser.add_argument('--generator', type=Path)
    parser.add_argument('--ear', action='store_true', help='Validate ADM with EBU EAR in the current Python environment')
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    if args.generator:
        generator = args.generator.resolve(strict=True)
    else:
        subprocess.run(['cargo', 'build', '--locked', '--release', '-p', 'macin-atmos',
                        '--example', 'synthetic_master'], cwd=root, check=True)
        metadata = json.loads(subprocess.check_output(
            ['cargo', 'metadata', '--locked', '--format-version', '1', '--no-deps'], cwd=root))
        generator = Path(metadata['target_directory']) / 'release/examples/synthetic_master'
        if not generator.exists():
            generator = generator.with_suffix('.exe')
    version = run([args.conversion_tool, '--version'])
    report = {'schema': 'macin-format.interop.v1',
              'conversion_tool_version': version.stdout.strip().replace(str(args.conversion_tool), args.conversion_tool.name),
              'generator_sha256': hashlib.sha256(generator.read_bytes()).hexdigest(), 'cases': []}
    if args.ear:
        import importlib.metadata
        report['ear_version'] = importlib.metadata.version('ear')
    with tempfile.TemporaryDirectory(prefix='macin-format-interop-') as directory:
        scratch = Path(directory)
        for rate in (48000, 96000):
            for profile in ('standard', 'logic', 'home', '3dof'):
                name = '{}-{}'.format(profile, rate)
                output = scratch / (name + '.wav' if profile in ('standard', 'logic') else name)
                generated = run([generator, output, profile, rate])
                if generated.returncode:
                    raise RuntimeError(generated.stderr)
                source = output if output.is_file() else output / 'master.atmos'
                pcm = wave(source)[2] if output.is_file() else (output / 'master.atmos.audio').read_bytes()[68:]
                converted = scratch / (name + '-normalized.wav')
                result = run([args.conversion_tool, '--pm_in', source,
                              '--output_path', converted, '--output_format', 'wav',
                              '--target_sample_rate', rate])
                item = {'profile': profile, 'sample_rate': rate,
                        'conversion_exit': result.returncode,
                        'pcm_sha256': hashlib.sha256(pcm).hexdigest()}
                if result.returncode == 0:
                    channels, actual_rate, actual_pcm = wave(converted)
                    item.update(channels=channels, converted_sample_rate=actual_rate,
                                converted_frames=len(actual_pcm) // (channels * 3),
                                pcm_identical=actual_pcm == pcm)
                else:
                    item['conversion_error'] = (result.stdout + result.stderr).replace(directory, '<temp>')[-1600:]
                if args.atmos_info:
                    info = run([args.atmos_info, '--input', source, '--validate', '1'])
                    item['atmos_info_exit'] = info.returncode
                    if info.returncode:
                        item['atmos_info_error'] = (info.stdout + info.stderr).replace(directory, '<temp>')[-1200:]
                if args.mradm and output.is_file():
                    inspected = run([args.mradm, 'inspect', source])
                    item['mradm_exit'] = inspected.returncode
                if args.ear and output.is_file():
                    check = run([sys.executable, '-c', (
                        "import sys; from ear.fileio import openBw64; "
                        "from ear.fileio.adm.adm import ADM; "
                        "from ear.fileio.adm.common_definitions import load_common_definitions; "
                        "from ear.fileio.adm.xml import load_axml_string; "
                        "from ear.fileio.adm.chna import load_chna_chunk; "
                        "\nwith openBw64(sys.argv[1]) as f:\n"
                        " a=ADM(); load_common_definitions(a)\n"
                        " load_axml_string(a,f.axml); load_chna_chunk(a,f.chna); a.validate()\n"
                        " print(len(a.audioObjects),len(a.audioTrackUIDs))\n"
                    ), source])
                    item['ear_exit'] = check.returncode
                    if check.returncode:
                        item['ear_error'] = (check.stdout + check.stderr).replace(directory, '<temp>')[-1800:]
                item['limitations'] = []
                converted_ok = (result.returncode == 0 and item.get('pcm_identical')
                                and item['converted_sample_rate'] == rate
                                and item['converted_frames'] == rate * 2)
                if result.returncode and profile == 'standard' and "missing 'RIFF/RF64' chunk" in item.get('conversion_error', ''):
                    item['limitations'].append('Conversion Tool does not recognize BW64')
                elif result.returncode and profile == '3dof' and 'Invalid file version 0.6.0' in item.get('conversion_error', ''):
                    item['limitations'].append('Conversion Tool supports DAMF through 0.5 only')
                elif not converted_ok:
                    item['unexpected_conversion_failure'] = True
                if item.get('atmos_info_exit', 0) != 0:
                    error = item.get('atmos_info_error', '')
                    if profile == 'logic' and 'Content was not authored with Dolby tools' in error:
                        item['limitations'].append('atmos_info rejects non-Dolby creator metadata for this RF64 input')
                    elif profile == 'standard' and "missing 'RIFF/RF64' chunk" in error:
                        item['limitations'].append('atmos_info does not recognize BW64')
                    else:
                        item['unexpected_info_failure'] = True
                # Each profile requires an independent validator which supports that format.
                # Known tool rejections remain recorded; unknown failures are not skipped.
                independent = (item.get('ear_exit') == 0 if profile == 'standard'
                               else item.get('atmos_info_exit') == 0 if profile == '3dof'
                               else converted_ok)
                item['accepted'] = bool(independent and not item.get('unexpected_conversion_failure')
                                        and not item.get('unexpected_info_failure')
                                        and item.get('mradm_exit', 0) == 0
                                        and item.get('ear_exit', 0) == 0)
                report['cases'].append(item)
                print(name, 'accepted=', item['accepted'], 'conversion=', result.returncode,
                      'PCM=', item.get('pcm_identical'), 'limitations=', len(item['limitations']), flush=True)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    else:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if all(c['accepted'] for c in report['cases']) else 1


if __name__ == '__main__':
    raise SystemExit(main())
