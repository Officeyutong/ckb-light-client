import { ScriptLike } from "@ckb-ccc/core";
import { JsonRpcScript, JsonRpcTransformers } from "@ckb-ccc/core/advancedBarrel";
import { Num, Script } from "@ckb-ccc/core/barrel";

interface WorkerInitializeOptions {
    inputBuffer: SharedArrayBuffer;
    outputBuffer: SharedArrayBuffer;
    logLevel: string;
}
interface DbWorkerInitializeOptions extends WorkerInitializeOptions {
}

interface LightClientWorkerInitializeOptions extends WorkerInitializeOptions {
    networkFlag: NetworkFlag;
};

interface LightClientFunctionCall {
    name: string;
    args: any[];
};

type FetchResponse<T> =
    { status: "fetched"; data: T; } |
    { status: "fetching"; first_sent: bigint; } |
    { status: "added"; timestamp: bigint; } |
    { status: "not_found" };

export function transformFetchResponse<A, B>(input: FetchResponse<A>, fn: (arg: A) => B): FetchResponse<B> {
    if (input.status === "fetched") {
        return { status: "fetched", data: fn(input.data) };
    }
    return input;
}

type JsonRpcScriptType = "lock" | "type";

interface JsonRpcScriptStatus {
    script: JsonRpcScript;
    script_type: JsonRpcScriptType;
    block_number: Num;
}

interface ScriptStatus {
    script: ScriptLike,
    scriptType: JsonRpcScriptType,
    blockNumber: Num
}

export function scriptStatusTo(input: JsonRpcScriptStatus): ScriptStatus {
    return ({
        blockNumber: input.block_number,
        script: JsonRpcTransformers.scriptTo(input.script),
        scriptType: input.script_type
    })
}

export function scriptStatusFrom(input: ScriptStatus): JsonRpcScriptStatus {
    return ({
        block_number: input.blockNumber,
        script: JsonRpcTransformers.scriptFrom(input.script),
        script_type: input.scriptType
    })
}

interface NodeAddress {
    address: string;
    score: Num;
}

interface RemoteNodeProtocol {
    id: Num;
    version: string;
}

interface JsonRpcRemoteNode {
    version: string;
    node_id: string;
    addresses: NodeAddress[];
    connected_duration: Num;
    sync_state?: any;
    protocols: RemoteNodeProtocol[];

}

interface RemoteNode {
    version: string;
    nodeId: string;
    addresses: NodeAddress[];
    connestedDuration: Num;
    syncState?: any;
    protocols: RemoteNodeProtocol[];
}

export function remoteNodeTo(input: JsonRpcRemoteNode): RemoteNode {
    return ({
        addresses: input.addresses,
        connestedDuration: input.connected_duration,
        nodeId: input.node_id,
        protocols: input.protocols,
        version: input.version,
        syncState: input.sync_state
    })
}

interface JsonRpcLocalNodeProtocol {
    id: Num;
    name: string;
    support_version: string[];
}


interface LocalNodeProtocol {
    id: Num;
    name: string;
    supportVersion: string[];
}

export function localNodeProtocolTo(input: JsonRpcLocalNodeProtocol): LocalNodeProtocol {
    return ({
        id: input.id,
        name: input.name,
        supportVersion: input.support_version
    })
}

interface JsonRpcLocalNode {
    version: string;
    node_id: string;
    active: boolean;
    addresses: NodeAddress[];
    protocols: JsonRpcLocalNodeProtocol[];
    connections: bigint;
}

interface LocalNode {
    version: string;
    nodeId: string;
    active: boolean;
    addresses: NodeAddress[];
    protocols: LocalNodeProtocol[];
    connections: bigint;
}

export function localNodeTo(input: JsonRpcLocalNode): LocalNode {
    return ({
        nodeId: input.node_id,
        protocols: input.protocols.map(x => localNodeProtocolTo(x)),
        active: input.active,
        addresses: input.addresses,
        connections: input.connections,
        version: input.version
    })
}
type NetworkFlag = { type: "MainNet" } | { type: "TestNet" } | { type: "DevNet"; spec: string; config: string; };
export enum LightClientWasmSetScriptsCommand {
    All = 0,
    Partial = 1,
    Delete = 2,
}
export enum LightClientWasmOrder {
    Desc = 0,
    Asc = 1,
}
export type {
    LightClientFunctionCall,
    WorkerInitializeOptions,
    DbWorkerInitializeOptions,
    LightClientWorkerInitializeOptions,
    FetchResponse,
    JsonRpcScriptStatus,
    ScriptStatus,
    JsonRpcRemoteNode,
    RemoteNode,
    LocalNode,
    JsonRpcLocalNode,
    NetworkFlag
}
