import * as wasmModule from "./pkg";
self.onerror = event => {
    console.error(event);
}

self.onmessage = async (evt) => {
    const data = evt.data;
    wasmModule.set_shared_array(data.inputBuffer, data.outputBuffer);
    self.postMessage({});
    await wasmModule.main_loop(data.logLevel);
}
