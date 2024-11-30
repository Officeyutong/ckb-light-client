import { ClientFindCellsResponse, ClientFindTransactionsGroupedResponse, ClientFindTransactionsResponse, ClientIndexerSearchKeyLike, ClientIndexerSearchKeyTransactionLike, ClientTransactionResponse } from "@ckb-ccc/core/src/client";
import { FetchResponse, transformFetchResponse } from "./types";
import { JsonRpcTransformers } from "@ckb-ccc/core/src/client/jsonRpc/transformers";
import { Num, numFrom, NumLike, numToHex } from "@ckb-ccc/core/src/num";
import { Hex, hexFrom, HexLike, TransactionLike } from "@ckb-ccc/core/src/barrel";
const DEFAULT_BUFFER_SIZE = 50 * (1 << 20);
/**
 * A LightClient instance
 */
class LightClient {
    dbWorker: Worker | null
    lightClientWorker: Worker | null
    inputBuffer: SharedArrayBuffer
    outputBuffer: SharedArrayBuffer
    /**
     * Construct a LightClient instance.
     * inputBuffer and outputBuffer are buffers used for transporting data between database and light client. Set them to appropriate sizes.
     * @param inputBufferSize Size of inputBuffer
     * @param outputBufferSize Size of outputBuffer
     */
    constructor(inputBufferSize = DEFAULT_BUFFER_SIZE, outputBufferSize = DEFAULT_BUFFER_SIZE) {
        this.dbWorker = new Worker(new URL("./db.worker.ts", import.meta.url), { type: "module" });
        this.lightClientWorker = new Worker(new URL("./lightclient.worker.ts", import.meta.url), { type: "module" });
        this.inputBuffer = new SharedArrayBuffer(inputBufferSize);
        this.outputBuffer = new SharedArrayBuffer(outputBufferSize);

    }
    /**
     * Start the light client.
     */
    async start() {
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
    private invokeLightClientCommand(name: string, args?: any[]): Promise<any> {
        this.lightClientWorker.postMessage({
            name,
            args: args || []
        });
        return new Promise((res, rej) => {
            this.lightClientWorker.onmessage = (e) => res(e.data);
            this.lightClientWorker.onerror = (evt) => rej(evt);
        });
    }
    /**
     * Stop the light client instance.
     */
    async stop() {
        await this.invokeLightClientCommand("stop");
        this.dbWorker.terminate();
        this.lightClientWorker.terminate();
    }
    /**
     * Returns the header with the highest block number in the canonical chain
     * @returns HeaderView
     */
    async getTipHeader(): Promise<any> {
        return await this.invokeLightClientCommand("get_tip_header");
    }
    /**
     * Returns the genesis block
     * @returns BlockView
     */
    async getGenesisBlock(): Promise<any> {
        return await this.invokeLightClientCommand("get_genesis_block");
    }
    /**
     * Returns the information about a block header by hash.
     * @param hash the block hash, equal to Vec<u8> in Rust
     * @returns HeaderView
     */
    async getHeader(hash: HexLike): Promise<any> {
        return await this.invokeLightClientCommand("get_header", [hexFrom(hash)]);
    }
    /**
     * Fetch a header from remote node. If return status is not_found will re-sent fetching request immediately.
     * @param hash the block hash, equal to Vec<u8> in Rust
     * @returns FetchHeaderResponse
     */
    async fetchHeader(hash: HexLike): Promise<FetchResponse<any>> {
        return await this.invokeLightClientCommand("fetch_header", [hexFrom(hash)]);
    }
    /**
     * See https://github.com/nervosnetwork/ckb/tree/develop/rpc#method-estimate_cycles
     * @param tx The transaction
     * @returns Estimate cycles
     */
    async estimateCycles(tx: TransactionLike): Promise<Num> {
        return (await this.invokeLightClientCommand("estimate_cycles", [JsonRpcTransformers.transactionFrom(tx)]) as any).cycles;
    }
    /**
     * Returns the local node information.
     * @returns LocalNode
     */
    async localNodeInfo(): Promise<any> {
        return await this.invokeLightClientCommand("local_node_info");
    }
    /**
     * Returns the connected peers' information.
     * @returns 
     */
    async getPeers(): Promise<any> {
        return await this.invokeLightClientCommand("get_peers");
    }
    /**
     * Set some scripts to filter
     * @param scripts Array of script status
     * @param command An optional enum parameter to control the behavior of set_scripts
     */
    async setScripts(scripts: any, command: any): Promise<void> {
        await this.invokeLightClientCommand("set_scripts", [scripts, command]);
    }
    /**
     * Get filter scripts status
     */
    async getScripts(): Promise<any[]> {
        return await this.invokeLightClientCommand("get_scripts");
    }
    /**
     * See https://github.com/nervosnetwork/ckb-indexer#get_cells
     * @param searchKey 
     * @param order 
     * @param limit 
     * @param afterCursor 
     */
    async getCells(
        searchKey: ClientIndexerSearchKeyLike,
        order?: "asc" | "desc",
        limit?: NumLike,
        afterCursor?: string
    ): Promise<ClientFindCellsResponse> {
        return JsonRpcTransformers.findCellsResponseTo(await this.invokeLightClientCommand("get_cells", [
            JsonRpcTransformers.indexerSearchKeyFrom(searchKey),
            order ?? "asc",
            numToHex(limit ?? 10),
            afterCursor
        ]));
    }
    /**
     * See https://github.com/nervosnetwork/ckb-indexer#get_transactions
     * @param searchKey 
     * @param order 
     * @param limit 
     * @param afterCursor 
     * @returns 
     */
    async getTransactions(
        searchKey: ClientIndexerSearchKeyTransactionLike,
        order?: "asc" | "desc",
        limit?: NumLike,
        afterCursor?: string
    ): Promise<ClientFindTransactionsResponse | ClientFindTransactionsGroupedResponse> {
        return JsonRpcTransformers.findTransactionsResponseTo(
            await this.invokeLightClientCommand(
                "get_transactions",
                [
                    JsonRpcTransformers.indexerSearchKeyTransactionFrom(searchKey),
                    order ?? "asc",
                    numToHex(limit ?? 10),
                    afterCursor
                ]
            )
        );
    }
    /**
     * See https://github.com/nervosnetwork/ckb-indexer#get_cells_capacity
     * @param searchKey 
     * @returns 
     */
    async getCellsCapacity(searchKey: ClientIndexerSearchKeyLike): Promise<Num> {
        return numFrom(((await this.invokeLightClientCommand("get_cells_capacity", [JsonRpcTransformers.indexerSearchKeyFrom(searchKey)])) as any).capacity);
    }
    /**
     * Submits a new transaction and broadcast it to network peers
     * @param tx Transaction
     * @returns H256
     */
    async sendTransaction(tx: TransactionLike): Promise<Hex> {
        return hexFrom(await this.invokeLightClientCommand("send_transaction", [JsonRpcTransformers.transactionFrom(tx)]));
    }
    /**
     * Returns the information about a transaction by hash, the block header is also returned.
     * @param txHash the transaction hash
     * @returns 
     */
    async getTransaction(txHash: HexLike): Promise<ClientTransactionResponse> {
        return JsonRpcTransformers.transactionResponseTo(await this.invokeLightClientCommand("get_transaction", [hexFrom(txHash)]));
    }
    /**
     * Fetch a transaction from remote node. If return status is not_found will re-sent fetching request immediately.
     * @param txHash the transaction hash
     * @returns 
     */
    async fetchTransaction(txHash: HexLike): Promise<FetchResponse<ClientTransactionResponse>> {
        return transformFetchResponse<any, ClientTransactionResponse>(await this.invokeLightClientCommand("fetch_transaction", [hexFrom(txHash)]), JsonRpcTransformers.transactionResponseTo);
    }

}

export default LightClient;
