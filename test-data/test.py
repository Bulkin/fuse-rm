import unittest

import glob
import json
import os
import shutil
import time
from datetime import datetime
from threading import Thread
from subprocess import Popen, PIPE, check_output
from pathlib import Path

ROOT = Path(__file__).parent
SRC_DIR = ROOT / 'source'
TARGET_DIR = ROOT / 'target'

class Test(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls._fuserm = Popen(['cargo', 'run', '--', SRC_DIR, TARGET_DIR],
                            stdout=PIPE)
        cls._fuserm_output = []
        while True:
            line = cls._fuserm.stdout.readline()
            if line.startswith(b'Waiting for Ctrl-C'):
                break
            elif cls._fuserm.poll is not None:
                raise RuntimeError('fuse-rm failed to start')
            else:
                print(line)
        cls._fuserm_thread = Thread(target=cls.capture_fuserm_output)
        cls._fuserm_thread.start()
        os.chdir(TARGET_DIR)

    @classmethod
    def capture_fuserm_output(cls):
        while True:
            if cls._fuserm.poll() is not None:
                break
            cls._fuserm_output.append(cls._fuserm.stdout.readline())
            #print(cls._fuserm_output[-1])

    @classmethod
    def tearDownClass(cls):
        cls._fuserm.terminate()

    def test_file_structure_root(self):
        root = set(check_output('stat -c "%s %n" *', shell=True).decode().split('\n'))
        self.assertSetEqual(root, { '',
                                    '0 dolor',
                                    '0 trash',
                                    '126501 ipsum.pdf',
                                    '4091 lorem.epub' })

    def test_file_structure_subdir(self):
        dir = set(check_output(['stat -c "%s %n" dolor/*'], shell=True).decode().split('\n'))
        self.assertSetEqual(dir, { '',
                                   '30875 dolor/ipsum.epub',
                                   '28859 dolor/lorem.pdf' })

    def test_rename(self):
        test_file = Path('ipsum.pdf')
        test_file_sz = test_file.stat().st_size
        new_file = test_file.rename(Path('dolor') / test_file.name)
        self.assertEqual(test_file_sz, new_file.stat().st_size)
        self.assertFalse(test_file.exists())
        new_file.rename(test_file)
        self.assertEqual(test_file_sz, test_file.stat().st_size)

    def test_mkdir(self):
        test_dir = Path('test_dir')
        test_dir.mkdir()
        self.assertTrue(test_dir.exists())
        test_dir_2 = test_dir / 't1'
        test_dir_2.mkdir()
        self.assertRaisesRegex(OSError, 'not empty', test_dir.rmdir)
        test_dir_2.rmdir()
        test_dir.rmdir()

    def test_add_epub(self):
        test_file = Path('ipsum.epub')
        self.assertFalse(test_file.exists())
        shutil.copyfile(Path('..') / test_file, test_file)

        self.test_file_structure_subdir()
        self.assertTrue(test_file.exists())

        test_file.unlink()
        self.assertFalse(test_file.exists())

    def test_add_pdf_subdir(self):
        test_file = Path('dolor/ipsum.pdf')
        src_file = Path('..') / test_file.name
        self.assertFalse(test_file.exists())
        shutil.copyfile(src_file, test_file)

        self.test_file_structure_root()
        self.assertEqual(test_file.stat().st_size, src_file.stat().st_size)

        test_file.unlink()
        self.assertFalse(test_file.exists())

    def test_add_unsupported(self):
        test_file = Path('../lorem.txt')
        self.assertRaisesRegex(OSError, 'not implemented',
                               lambda: shutil.copyfile(test_file, test_file.name))

    def test_rm_recursive(self):
        test_dir = Path('test-dir/deeper/')
        test_dir.mkdir(parents=True, exist_ok=True)
        self.assertTrue(test_dir.exists())
        shutil.copyfile('../lorem.epub', test_dir / 'a')
        shutil.copyfile('../lorem.pdf', test_dir / 'b')
        shutil.rmtree(test_dir.parent)
        self.assertFalse(test_dir.exists())

    def test_dir_with_dot(self):
        # make KOReader's side cars go away
        test_dir = Path('test-dir.sdr')
        self.assertRaisesRegex(OSError, 'not implemented',
                               test_dir.mkdir)

    # TODO: file moved to trash -> file exists error
    def test_last_modified(self):
        curtime = datetime.now()

        test_file = Path('ipsum.epub')
        self.assertFalse(test_file.exists())
        shutil.copyfile(Path('..') / test_file, test_file)

        newest = max(glob.iglob(str(SRC_DIR) + '/*.metadata'),
                     key=os.path.getctime)
        print(newest)

        with open(newest) as f:
            metadata = json.load(f)

        self.assertTrue(isinstance(metadata['lastModified'], str))
        modtime = datetime.fromtimestamp(int(metadata['lastModified']) // 1000)

        self.assertTrue((modtime - curtime).total_seconds() < 1)

        test_file.unlink()
        self.assertFalse(test_file.exists())
