import { expect, test } from "@jest/globals";
import { cccOrderToLightClientWasmOrder, FetchResponse, GetTransactionsResponse, lightClientGetTransactionsResultTo, LightClientLocalNode, LightClientOrder, LightClientRemoteNode, LightClientScriptStatus, LightClientTxWithCell, LightClientTxWithCells, LocalNode, localNodeTo, RemoteNode, remoteNodeTo, scriptStatusFrom, scriptStatusTo, transformFetchResponse, TxWithCell, TxWithCells } from "./types";
import { Transaction } from "@ckb-ccc/core";
test("test transform fetch response", () => {
    const a: FetchResponse<number> = { status: "fetched", data: 1 };
    const b: FetchResponse<number> = { status: "added", timestamp: 0n };
    const fn = (a: number) => a + 1;
    expect(transformFetchResponse(a, fn)).toStrictEqual({ status: "fetched", data: 2 });
    expect(transformFetchResponse(b, fn)).toStrictEqual(b);
})

test("test scriptStatusTo/From", () => {
    const raw: LightClientScriptStatus = {
        script: {
            code_hash: "0xabcd1234",
            hash_type: "data",
            args: "0x0011223344"
        },
        script_type: "lock",
        block_number: "0x1234"
    };
    const transformed = scriptStatusTo(raw);

    expect(scriptStatusFrom(transformed)).toStrictEqual(raw);
});

test("test remoteNodeTo", () => {
    const raw: LightClientRemoteNode = {
        version: "1",
        node_id: "111",
        addresses: [
            { address: "test", score: 123n }
        ],
        connected_duration: 234n,
        protocols: [
            { id: 1n, version: "1" }
        ],
        sync_state: {
            proved_best_known_header: {
                compact_target: "0x12",
                dao: "0x04d0bc8a6028d30c0000c16ff286230066cbed490e00000000a38cdc1afbfe06",
                epoch: "0x11112222333333",
                extra_hash: "0x45",
                hash: "0x56",
                nonce: "0x67",
                number: "0x78",
                parent_hash: "0x89",
                proposals_hash: "0x90",
                timestamp: "0xab",
                transactions_root: "0xbc",
                version: "0xcd"
            }
        }
    };
    const newVal: RemoteNode = {
        version: "1",
        nodeId: "111",
        addresses: [
            { address: "test", score: 123n }
        ],
        connestedDuration: 234n,
        protocols: [
            { id: 1n, version: "1" }
        ],
        syncState: {
            provedBestKnownHeader: {
                compactTarget: 0x12n,
                dao: {
                    c: 924126743650684932n,
                    ar: 10000000000000000n,
                    s: 61369863014n,
                    u: 504116301100000000n
                },
                epoch: [3355443n, 8738n, 4369n],
                extraHash: "0x45",
                hash: "0x56",
                nonce: 0x67n,
                number: 0x78n,
                parentHash: "0x89",
                proposalsHash: "0x90",
                timestamp: 0xabn,
                transactionsRoot: "0xbc",
                version: 0xcdn
            },
            requestedBestKnownHeader: undefined
        }
    };
    expect(remoteNodeTo(raw)).toStrictEqual(newVal);
});

test("test localNodeTo", () => {
    const raw: LightClientLocalNode = {
        version: "aaa",
        node_id: "bbb",
        active: false,
        addresses: [
            { address: "111", score: 123n }
        ],
        protocols: [
            { id: 1n, name: "test", support_version: ["a", "b", "c"] }
        ],
        connections: 123n
    }
    const expectedVal: LocalNode = {
        version: "aaa",
        nodeId: "bbb",
        active: false,
        addresses: [
            { address: "111", score: 123n }
        ],
        protocols: [
            { id: 1n, name: "test", supportVersion: ["a", "b", "c"] }
        ],
        connections: 123n
    };
    expect(localNodeTo(raw)).toStrictEqual(expectedVal);
});

test("test cccOrderToLightClientWasmOrder", () => {
    expect(cccOrderToLightClientWasmOrder("asc")).toBe(LightClientOrder.Asc);
    expect(cccOrderToLightClientWasmOrder("desc")).toBe(LightClientOrder.Desc);
});

test("test lightClientGetTransactionsResultTo", () => {
    expect(lightClientGetTransactionsResultTo({ last_cursor: "0x1234", objects: [] })).toStrictEqual({ lastCursor: "0x1234", transactions: [] });
    expect(lightClientGetTransactionsResultTo({
        last_cursor: "0x1234", objects: [
            {
                block_number: "0x1234",
                io_index: 1234,
                io_type: "input",
                transaction: {
                    cell_deps: [],
                    header_deps: [],
                    inputs: [],
                    outputs: [],
                    outputs_data: [],
                    version: "0x123",
                    witnesses: ["0x"]
                },
                tx_index: 2222
            } as LightClientTxWithCell
        ]
    })).toStrictEqual({
        lastCursor: "0x1234",
        transactions: [
            {
                blockNumber: 0x1234n,
                ioIndex: 1234n,
                ioType: "input",
                transaction: Transaction.from({
                    cellDeps: [],
                    headerDeps: [],
                    inputs: [],
                    outputs: [],
                    outputsData: [],
                    version: 0x123n,
                    witnesses: ["0x"]
                }),
                txIndex: 2222n
            }
        ]
    } as GetTransactionsResponse<TxWithCell>);

    expect(lightClientGetTransactionsResultTo({
        last_cursor: "0x1234", objects: [
            {
                block_number: "0x1234",
                tx_index: 123,
                transaction: {
                    cell_deps: [],
                    header_deps: [],
                    inputs: [],
                    outputs: [],
                    outputs_data: [],
                    version: "0x123",
                    witnesses: ["0x"]
                },
                cells: [
                    ["input", 111]
                ]
            } as LightClientTxWithCells
        ]
    })).toStrictEqual({
        lastCursor: "0x1234",
        transactions: [
            {
                blockNumber: 0x1234n,
                txIndex: 123n,
                transaction: Transaction.from({
                    cellDeps: [],
                    headerDeps: [],
                    inputs: [],
                    outputs: [],
                    outputsData: [],
                    version: 0x123n,
                    witnesses: ["0x"]
                }),
                cells: [
                    ["input", 111]
                ]
            }
        ]
    } as GetTransactionsResponse<TxWithCells>);
});
