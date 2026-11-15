"""互操作检查器的工具限制与读回失败回归。"""
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import check_interop


class DamfReadbackTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='macin-format-interop-test-')
        self.addCleanup(temporary.cleanup)
        self.scratch = Path(temporary.name)
        self.args = SimpleNamespace(conversion_tool=Path('conversion-tool'), damf_format='atmos')

    def test_only_recognized_96_khz_output_limits_skip_readback(self):
        cases = (
            (96000, 'ERROR: Sample rate conversion from 96kHz .wav to 96kHz.atmos is not supported.', True),
            (96000, 'ERROR: Sample rate conversion from 96kHz .atmos to 96kHz.atmos is not supported.', True),
            (96000, 'ERROR: Failed to open output file.', False),
            (96000, 'ERROR: Sample rate conversion from 96kHz .wav to 48kHz.atmos is not supported.', False),
            (48000, 'ERROR: Sample rate conversion from 96kHz .wav to 96kHz.atmos is not supported.', False),
        )
        for index, (rate, error, skipped) in enumerate(cases):
            with self.subTest(rate=rate, error=error):
                result = subprocess.CompletedProcess([], 255, stdout='Progress - 10 %\n', stderr=error + '\n')
                with patch.object(check_interop, 'run', return_value=result) as run:
                    item = check_interop.damf_readback(
                        self.args, Path('mfadm'), self.scratch, f'case-{index}', Path('source'), rate, b'', 2)
                run.assert_called_once()
                self.assertEqual(run.call_args.args[0][-2:], ['--target_sample_rate', rate])
                self.assertEqual(item['conversion_exit'], 255)
                self.assertIn(error, item['conversion_error'])
                self.assertEqual('skipped' in item, skipped)
                if skipped:
                    self.assertNotIn('accepted', item)
                else:
                    self.assertIs(item['accepted'], False)

    def test_reader_failure_is_recorded_as_failed_acceptance(self):
        def run(command):
            if command[0] == self.args.conversion_tool:
                Path(command[command.index('--output_path') + 1]).write_text('manifest', encoding='utf-8')
                return subprocess.CompletedProcess(command, 0, stdout='', stderr='')
            return subprocess.CompletedProcess(
                command, 1, stdout='', stderr=f'mfadm: {self.scratch}/master.atmos.metadata: missing ID\n')

        with patch.object(check_interop, 'run', side_effect=run):
            item = check_interop.damf_readback(
                self.args, Path('mfadm'), self.scratch, 'reader-error', Path('source'), 48000, b'', 2)
        self.assertIs(item['accepted'], False)
        self.assertEqual(item['mfadm_exit'], 1)
        self.assertNotIn('skipped', item)
        self.assertIn('missing ID', item['mfadm_error'])
        self.assertNotIn(str(self.scratch), item['mfadm_error'])


if __name__ == '__main__':
    unittest.main()
