from wasi.wit_world import exports
import time

class Run(exports.Run):
    def run(self) -> None:
        while True:
            print("Hello, world from python!")
            time.sleep(1)
