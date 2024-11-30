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

export type {
    LightClientFunctionCall,
    WorkerInitializeOptions,
    DbWorkerInitializeOptions,
    LightClientWorkerInitializeOptions,
    FetchResponse
}
