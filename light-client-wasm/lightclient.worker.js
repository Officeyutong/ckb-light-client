import * as wasmModule from "./pkg";
onerror = err => {
    console.error(err);
}
let loaded = false;
onmessage = async (evt) => {
    const data = evt.data;
    if (!loaded) {
        wasmModule.set_shared_array(data.inputBuffer, data.outputBuffer);
        await wasmModule.light_client(data.netName, data.logLevel);
        self.postMessage({});
        loaded = true;
        return;
    }
    self.postMessage(wasmModule[evt.data.name](...evt.data.args))
};
