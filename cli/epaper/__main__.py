import argparse
import os
import sys

import serial

from . import console, image, protocol as p
from .transport import DeviceError, Transport


def parser():
    root = argparse.ArgumentParser(prog='epaper')
    root.add_argument('--port')
    root.add_argument('--verbose', action='store_true')
    sub = root.add_subparsers(dest='action', required=True)
    for name in ('ping', 'info', 'sleep', 'bootsel'):
        sub.add_parser(name)
    clear = sub.add_parser('clear')
    clear.add_argument('--black', action='store_true')
    mode = sub.add_parser('mode')
    mode.add_argument('mode', choices=('picture', 'console'))
    write = sub.add_parser('write')
    write.add_argument('text', help=r'text; literal \n becomes newline')
    draw = sub.add_parser('draw')
    source = draw.add_mutually_exclusive_group(required=True)
    source.add_argument('image', nargs='?')
    source.add_argument('--testcard', action='store_true')
    draw.add_argument('--fit', choices=('fill', 'fit', 'stretch'), default='fit')
    draw.add_argument('--rotate', type=int, choices=(0, 90, 180, 270), default=0)
    mono = draw.add_mutually_exclusive_group()
    mono.add_argument('--dither', action='store_true')
    mono.add_argument('--threshold', type=int, default=128)
    draw.add_argument('--invert', action='store_true')
    draw.add_argument('--partial', action='store_true')
    terminal = sub.add_parser('console')
    terminal.add_argument('--stdin', action='store_true')
    terminal.add_argument('--echo', action='store_true')
    terminal.add_argument('--cols', type=int, default=100)
    terminal.add_argument('--rows', type=int, default=30)
    terminal.add_argument('command', nargs=argparse.REMAINDER)
    return root


def main(argv=None):
    args = parser().parse_args(argv)
    try:
        with Transport(args.port, args.verbose) as device:
            if args.action in ('ping', 'info', 'sleep', 'bootsel'):
                constructor = {'ping': p.ping, 'info': p.info, 'sleep': p.sleep,
                               'bootsel': p.reset_bootsel}[args.action]
                response = device.request(*constructor())
                if response:
                    print(response.decode('ascii', errors='replace'))
            elif args.action == 'mode':
                device.request(*p.set_mode(args.mode == 'console'))
            elif args.action == 'clear':
                device.request(*p.clear(args.black))
            elif args.action == 'write':
                device.request(*p.console_write(args.text.replace('\\n', '\n').encode()))
            elif args.action == 'draw':
                options = dict(fit=args.fit, rotate=args.rotate, dither=args.dither,
                               threshold=args.threshold, invert=args.invert)
                data = image.pack(image.testcard(), **options) if args.testcard else image.load(args.image, **options)
                device.request(*p.img_begin())
                for offset in range(0, len(data), 4090):
                    chunk = data[offset:offset + 4090]
                    device.request(*p.img_data(offset, chunk))
                    print(f'\rUploaded {offset + len(chunk)}/{len(data)} bytes', end='', file=sys.stderr, flush=True)
                print('\nRefreshing...', file=sys.stderr)
                device.request(*p.img_end(args.partial))
            elif args.action == 'console':
                command = args.command
                if command[:1] == ['--']:
                    command = command[1:]
                if args.stdin:
                    if command:
                        raise ValueError('--stdin cannot be combined with a command')
                    return console.pipe_stdin(device)
                return console.run(device, command or [os.environ.get('SHELL', '/bin/sh')],
                                   args.echo, args.cols, args.rows)
        return 0
    except KeyboardInterrupt:
        return 130
    except (DeviceError, serial.SerialException, TimeoutError, OSError, ValueError) as exc:
        print(f'epaper: {exc}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
