import struct
import tempfile
import unittest
from pathlib import Path
import research


class ResearchTests(unittest.TestCase):
    def test_alias_address_resolution_and_unknown_names(self):
        catalog = {'symbols': {'charge': {'address': '004e79e0'}}}
        for value in ('charge', 'FUN_004e79e0', '0x4e79e0'):
            self.assertEqual(research.resolve(value, catalog), '004e79e0')
        with self.assertRaises(ValueError):
            research.resolve('charg_typo', catalog)

    def test_excerpts_report_unread_lines(self):
        self.assertEqual(research.bounded(['a','b','c'], 2, 1),
                         ['2: b', '[truncated: 1 more lines; resume with --start 3]'])
        self.assertEqual(research.bounded(['a','b'], 2, 5), ['2: b'])
        with self.assertRaises(ValueError):
            research.bounded(['a'], 0, 10)

    def test_source_drift_is_detected(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            exe = folder/'reference.exe'
            exe.write_bytes(b'original')
            source = folder/'004e79e0_charge.c'
            source.write_text('first evidence', encoding='utf-8')
            catalog = {'reference': {'exe_sha256': research.digest(exe)},
                       'symbols': {'charge': {'address':'004e79e0', 'decompile_sha256':research.digest(source)}},
                       'features': {}}
            self.assertEqual(research.verify(catalog, research.sources(folder), exe), [])
            source.write_text('changed evidence', encoding='utf-8')
            self.assertIn('changed', research.verify(catalog, research.sources(folder), exe)[0])
            exe.write_bytes(b'different build')
            self.assertIn('SHA256', research.verify(catalog, research.sources(folder), exe)[0])

    def test_duplicate_addresses_are_not_silently_overridden(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            (folder/'004e79e0_first.c').touch()
            (folder/'004e79e0_second.c').touch()
            with self.assertRaises(ValueError):
                research.sources(folder)

    def test_pe_va_mapping_respects_file_backed_boundaries(self):
        data = bytearray(1024)
        data[:2] = b'MZ'
        struct.pack_into('<I', data, 0x3c, 0x80)
        data[0x80:0x84] = b'PE\0\0'
        struct.pack_into('<HH', data, 0x84, 0x14c, 1)
        struct.pack_into('<H', data, 0x94, 224)
        struct.pack_into('<H', data, 0x98, 0x10b)
        struct.pack_into('<I', data, 0xb4, 0x400000)
        struct.pack_into('<IIII', data, 0x80+24+224+8, 100, 0x1000, 4, 512)
        data[512:516] = b'ABCD'
        self.assertEqual(research.pe_bytes(data, 0x401001, 100), b'BCD')
        with self.assertRaises(ValueError):
            research.pe_bytes(data, 0x401004, 1)
        data[:2] = b'XX'
        with self.assertRaises(ValueError):
            research.pe_bytes(data, 0x401000, 1)


if __name__ == '__main__':
    unittest.main()
