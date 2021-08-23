import unittest

import os
import shutil
import time
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
            if line.startswith(b'Waiting for Ctrl-C') or line == b'':
                break
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

    def test_file_structure(self):
        root = set(check_output('stat -c "%s %n" *', shell=True).decode().split('\n'))
        dir = set(check_output(['stat -c "%s %n" dolor/*'], shell=True).decode().split('\n'))
        self.assertSetEqual(root, { '', '0 dolor',
                                 '126501 ipsum.pdf',
                                 '4091 lorem.epub' })
        self.assertSetEqual(dir, { '', '30875 dolor/ipsum.epub',
                                '28859 dolor/lorem.pdf' })


    def test_rename(self):
        test_file = Path('ipsum.pdf')
        test_file_sz = test_file.stat().st_size
        new_file = test_file.rename(Path('dolor') / test_file.name)
        self.assertEqual(test_file_sz, new_file.stat().st_size)
        self.assertFalse(test_file.exists())
        new_file.rename(test_file)
        self.assertEqual(test_file_sz, test_file.stat().st_size)
