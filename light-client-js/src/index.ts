const DEFAULT_BUFFER_SIZE = 50 * (1 << 20);

class LightClient {
    dbWorker: Worker
    lightClientWorker: Worker
    inputBuffer: SharedArrayBuffer
    outputBuffer: SharedArrayBuffer
    constructor(inputBufferSize = DEFAULT_BUFFER_SIZE, outputBufferSize = DEFAULT_BUFFER_SIZE) {
        this.dbWorker = new Worker(new URL("./db.worker.ts", import.meta.url), { type: "module" });
        this.lightClientWorker = new Worker(new URL("./lightclient.worker.ts", import.meta.url), { type: "module" });
        this.inputBuffer = new SharedArrayBuffer(inputBufferSize);
        this.outputBuffer = new SharedArrayBuffer(outputBufferSize);

    }
    async start() {
        console.log("test");
        this.dbWorker.postMessage({
            inputBuffer: this.inputBuffer,
            outputBuffer: this.outputBuffer,
            logLevel: "info"
        });
        this.lightClientWorker.postMessage({
            inputBuffer: this.inputBuffer,
            outputBuffer: this.outputBuffer,
            netName: "dev",
            logLevel: "info"
        });
        await new Promise<void>((res, rej) => {
            this.dbWorker.onmessage = () => res();
            this.dbWorker.onerror = (evt) => rej(evt);
        });
        await new Promise<void>((res, rej) => {
            this.lightClientWorker.onmessage = () => res();
            this.lightClientWorker.onerror = (evt) => rej(evt);
        });
    }
    async invokeCommand(name: string, args?: any[]): Promise<any> {
        this.lightClientWorker.postMessage({
            name,
            args: args || []
        });
        return new Promise((res, rej) => {
            this.lightClientWorker.onmessage = (e) => res(e.data);
            this.lightClientWorker.onerror = (evt) => rej(evt);
        });
    }
}

export default LightClient;
