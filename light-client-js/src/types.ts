interface WorkerInitializeOptions {
    inputBuffer: SharedArrayBuffer;
    outputBuffer: SharedArrayBuffer;
    logLevel: string;
}
interface DbWorkerInitializeOptions extends WorkerInitializeOptions {
}

interface LightClientWorkerInitializeOptions extends WorkerInitializeOptions {
    netName: string;
};

interface LightClientFunctionCall {
    name: string;
    args: any[];
};

type FetchHeaderResponse =
    { status: "fetched"; data: any; } |
    { status: "fetching"; first_sent: bigint; } |
    { status: "added"; timestamp: bigint; } |
    { status: "not_found" };

export type {
    LightClientFunctionCall,
    WorkerInitializeOptions,
    DbWorkerInitializeOptions,
    LightClientWorkerInitializeOptions,
    FetchHeaderResponse
}
