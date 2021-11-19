#!/usr/bin/env python3

import os
import sys
import json
import shlex
import shutil
import pathlib
import tempfile
import contextlib
import subprocess
from typing import List

import trio
import colorama
from colorama import Fore


BOOTSTRAP_COLOR = Fore.WHITE
REPLICATOR_COLOR = Fore.YELLOW


class OrgNodeError(RuntimeError):
    pass


# The default context manager for trio.Process waits until the process finishes whereas we want to terminate it ourselves.
@contextlib.asynccontextmanager
async def org_node_process(org_node_cmd: List[str]) -> trio.Process:
    process = await trio.open_process(org_node_cmd, stdout=subprocess.PIPE)
    try:
        yield process
    finally:
        process.terminate()


async def org_node_log_entries(process: trio.Process):
    buffer = b''
    async for chunk in process.stdout:
        lines = chunk.split(b'\n')
        for line in lines:
            if line == b'':
                continue
            if len(buffer) > 0:
                buffer += line
                line = buffer
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                # An entire log entry may not fit into one chunk.  We continue with the next chunk
                break
            else:
                buffer = b''
            yield entry


async def bootstrap_process(process: trio.Process, bootstrap_ready_event: trio.Event, cancel_scope: trio.CancelScope) -> None:
    expected = """
Setting ref "refs/namespaces/hnrkjajuucc6zp5eknt3s9xykqsrus44cjimy/refs/heads/master" -> 2ceeb04ce6379bff1eba34ee9b498209f6435ae6
Setting ref "refs/namespaces/hnrkjajuucc6zp5eknt3s9xykqsrus44cjimy/refs/remotes/hyn9diwfnytahjq8u3iw63h9jte1ydcatxax3saymwdxqu1zo645pe/heads/master" -> 2ceeb04ce6379bff1eba34ee9b498209f6435ae6
Setting ref "refs/namespaces/hnrkjajuucc6zp5eknt3s9xykqsrus44cjimy/HEAD" -> "refs/namespaces/hnrkjajuucc6zp5eknt3s9xykqsrus44cjimy/refs/heads/master"

Setting ref "refs/namespaces/hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto/refs/heads/master" -> acce81a6568ec4342586a71a314485408c264068
Setting ref "refs/namespaces/hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto/refs/remotes/hyn9diwfnytahjq8u3iw63h9jte1ydcatxax3saymwdxqu1zo645pe/heads/master" -> acce81a6568ec4342586a71a314485408c264068
Setting ref "refs/namespaces/hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto/HEAD" -> "refs/namespaces/hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto/refs/heads/master"
"""
    expected_lines = set(l for l in expected.split('\n') if l)
    remaining_expected_lines = len(expected_lines)
    async for entry in org_node_log_entries(process):
        sys.stdout.buffer.write(BOOTSTRAP_COLOR.encode() + json.dumps(entry).encode() + Fore.RESET.encode() + b'\n')
        sys.stdout.flush()
        if entry['severity'] == 'ERROR':
            raise OrgNodeError(entry['message'])
        if entry['message'] in expected_lines:
            remaining_expected_lines -= 1
        if remaining_expected_lines == 0:
            bootstrap_ready_event.set()
    cancel_scope.cancel()


async def replicator_process(process: trio.Process, bootstrap_ready_event: trio.Event, cancel_scope: trio.CancelScope) -> None:
    expected = """
Setting ref "refs/namespaces/hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto/refs/heads/master" -> acce81a6568ec4342586a71a314485408c264068
Setting ref "refs/namespaces/hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto/refs/remotes/hyn9diwfnytahjq8u3iw63h9jte1ydcatxax3saymwdxqu1zo645pe/heads/master" -> acce81a6568ec4342586a71a314485408c264068
Setting ref "refs/namespaces/hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto/HEAD" -> "refs/namespaces/hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto/refs/heads/master"
"""
    expected_lines = set(l for l in expected.split('\n') if l)
    remaining_expected_lines = len(expected_lines)
    async for entry in org_node_log_entries(process):
        sys.stdout.buffer.write(REPLICATOR_COLOR.encode() + json.dumps(entry).encode() + Fore.RESET.encode() + b'\n')
        sys.stdout.flush()
        if entry['severity'] == 'ERROR':
            raise OrgNodeError(entry['message'])
        if entry['message'] in expected_lines:
            remaining_expected_lines -= 1
        if remaining_expected_lines == 0:
            break
    cancel_scope.cancel()


def make_shell_command(shell_words: List[str]) -> str:
    return ' '.join(shlex.quote(shell_word) for shell_word in shell_words)


async def async_main(org_node_executable_path: pathlib.Path, workdir_path: pathlib.Path) -> None:
    bootstrap_workdir_path = workdir_path / 'bootstrap'
    bootstrap_identity_path = bootstrap_workdir_path / 'identity'
    bootstrap_git_path = bootstrap_workdir_path / 'git'
    bootstrap_node_cmd = [
        str(org_node_executable_path),
        '--subgraph', 'https://api.thegraph.com/subgraphs/name/radicle-dev/radicle-orgs',
        '--rpc-url', 'wss://eth-rinkeby.alchemyapi.io/v2/1T6h-0rxu7SRzKEtmukIoxaJOXazLDNs',
        '--identity', str(bootstrap_identity_path),
        '--root', str(bootstrap_workdir_path),
        '--orgs', '0x0000000000000000000000000000000000000000',
        '--urns', 'rad:git:hnrkjajuucc6zp5eknt3s9xykqsrus44cjimy,rad:git:hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto',
        '--listen', '127.0.0.1:8776',
        '--web-server-listen', '127.0.0.1:8336',
        '--log-format', 'gcp',
    ]
    print('Bootstrap node command:', make_shell_command(bootstrap_node_cmd))

    with tempfile.TemporaryDirectory() as replicator_workdir_path:
        replicator_workdir_path = pathlib.Path(replicator_workdir_path)
        shutil.copyfile(workdir_path / 'replicator' / 'identity', replicator_workdir_path / 'identity')
        replicator_identity_path = replicator_workdir_path / 'identity'
        replicator_git_path = replicator_workdir_path / 'git'
        # The tested code behaves differently if there are some files present in the replicator working directory.  Make sure we start from scratch each time to stay deterministic.
        replicator_node_cmd = [
            str(org_node_executable_path),
            '--subgraph', 'https://api.thegraph.com/subgraphs/name/radicle-dev/radicle-orgs',
            '--rpc-url', 'wss://eth-rinkeby.alchemyapi.io/v2/1T6h-0rxu7SRzKEtmukIoxaJOXazLDNs',
            '--identity', str(replicator_identity_path),
            '--root', str(replicator_workdir_path),
            '--orgs', '0x0000000000000000000000000000000000000000',
            '--urns', 'rad:git:hnrkjajuucc6zp5eknt3s9xykqsrus44cjimy,rad:git:hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto',
            '--bootstrap', 'hybju4ci46qn4nj844ogabz45y5o98nqrdxs3pmksj5fok9wgwa1bq@127.0.0.1:8776',
            '--listen', '127.0.0.1:8777',
            '--web-server-listen', '127.0.0.1:8337',
            '--log-format', 'gcp',
        ]
        print('Replicator node command:', make_shell_command(replicator_node_cmd))

        os.environ['RUST_LOG'] = 'DEBUG'
        async with org_node_process(bootstrap_node_cmd) as bootstrap:
            bootstrap_ready_event = trio.Event()
            async with trio.open_nursery() as nursery:
                nursery.start_soon(bootstrap_process, bootstrap, bootstrap_ready_event, nursery.cancel_scope)
                await bootstrap_ready_event.wait()
                async with org_node_process(replicator_node_cmd) as replicator:
                    await replicator_process(replicator, bootstrap_ready_event, nursery.cancel_scope)

        assert bootstrap_git_path.joinpath('refs/namespaces/hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto/refs/remotes/hyn9diwfnytahjq8u3iw63h9jte1ydcatxax3saymwdxqu1zo645pe/heads/master').is_file()
        assert replicator_git_path.joinpath('refs/namespaces/hnrkbtw9t1of4ykjy6er4qqwxtc54k9943eto/refs/remotes/hyn9diwfnytahjq8u3iw63h9jte1ydcatxax3saymwdxqu1zo645pe/heads/master').is_file()



def main():
    org_node_executable_path = pathlib.Path(__file__).parent.parent.absolute().joinpath('target/debug/radicle-org-node')
    workdir_path = pathlib.Path(__file__).parent.joinpath('input').absolute()
    trio.run(async_main, org_node_executable_path, workdir_path)



if __name__ == '__main__':
    sys.exit(main())
