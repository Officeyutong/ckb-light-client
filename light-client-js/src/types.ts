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

export type {
    LightClientFunctionCall,
    WorkerInitializeOptions,
    DbWorkerInitializeOptions,
    LightClientWorkerInitializeOptions
}
