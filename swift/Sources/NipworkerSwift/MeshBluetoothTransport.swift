#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth
import Foundation

/// Foreground CoreBluetooth byte transport for the Rust mesh engine.
///
/// This type intentionally contains no Nostr, Negentropy, subscription, or
/// cache logic. It advertises a nipworker-specific GATT service and forwards
/// characteristic values through `MeshNativeCodec`.
@MainActor
public final class MeshBluetoothTransport: NSObject {
    public static let serviceUUID = CBUUID(string: "7F4A0001-9B5D-4D3B-8E2A-4E4950574B52")
    public static let writeUUID = CBUUID(string: "7F4A0002-9B5D-4D3B-8E2A-4E4950574B52")
    public static let notifyUUID = CBUUID(string: "7F4A0003-9B5D-4D3B-8E2A-4E4950574B52")

    public var onPeerConnected: (@Sendable (_ peerID: String) -> Void)?
    public var onPeerDisconnected: (@Sendable (_ peerID: String) -> Void)?

    private let codec: MeshNativeCodec
    private lazy var central = CBCentralManager(delegate: self, queue: .main)
    private lazy var peripheral = CBPeripheralManager(delegate: self, queue: .main)
    private var discovered: [UUID: CBPeripheral] = [:]
    private var writeCharacteristics: [UUID: CBCharacteristic] = [:]
    private var subscribedCentrals: [UUID: CBCentral] = [:]
    private var notifyCharacteristic: CBMutableCharacteristic?
    private var pendingPeripheralWrites: [UUID: [Data]] = [:]
    private var pendingCentralWrites: [UUID: [Data]] = [:]
    private var running = false

    private init(codec: MeshNativeCodec) {
        self.codec = codec
        super.init()
    }

    public static func create(for manager: NostrManager) async -> MeshBluetoothTransport? {
        let handle = await manager.nativeHandleForMesh()
        guard let codec = MeshNativeCodec(engineHandle: handle) else { return nil }
        return MeshBluetoothTransport(codec: codec)
    }

    public func start() {
        running = true
        _ = central
        _ = peripheral
        refreshBluetoothState()
    }

    public func stop() {
        running = false
        central.stopScan()
        peripheral.stopAdvertising()
        for remote in discovered.values {
            central.cancelPeripheralConnection(remote)
        }
        discovered.removeAll()
        writeCharacteristics.removeAll()
        subscribedCentrals.removeAll()
        pendingPeripheralWrites.removeAll()
        pendingCentralWrites.removeAll()
    }

    private func refreshBluetoothState() {
        guard running else { return }
        if central.state == .poweredOn {
            central.scanForPeripherals(
                withServices: [Self.serviceUUID],
                options: [CBCentralManagerScanOptionAllowDuplicatesKey: false]
            )
        }
        if peripheral.state == .poweredOn {
            installServiceIfNeeded()
        }
    }

    private func installServiceIfNeeded() {
        guard notifyCharacteristic == nil else { return }
        let write = CBMutableCharacteristic(
            type: Self.writeUUID,
            properties: [.write, .writeWithoutResponse],
            value: nil,
            permissions: [.writeable]
        )
        let notify = CBMutableCharacteristic(
            type: Self.notifyUUID,
            properties: [.notify],
            value: nil,
            permissions: [.readable]
        )
        let service = CBMutableService(type: Self.serviceUUID, primary: true)
        service.characteristics = [write, notify]
        notifyCharacteristic = notify
        peripheral.add(service)
    }

    private func register(peerID: String, mtu: Int) {
        guard codec.peerConnected(peerID, mtu: mtu) else { return }
        onPeerConnected?(peerID)
    }

    private func receive(_ fragment: Data, from peerID: String) {
        guard codec.receive(fragment: fragment, from: peerID) else { return }
        drainOutbound(to: peerID)
    }

    private func drainOutbound(to peerID: String) {
        while let fragment = codec.popOutbound(for: peerID) {
            if let uuid = UUID(uuidString: peerID),
               let remote = discovered[uuid],
               let characteristic = writeCharacteristics[uuid] {
                guard remote.canSendWriteWithoutResponse else {
                    pendingPeripheralWrites[uuid, default: []].append(fragment)
                    return
                }
                remote.writeValue(fragment, for: characteristic, type: .withoutResponse)
                continue
            }
            if let uuid = UUID(uuidString: peerID),
               let central = subscribedCentrals[uuid],
               let notifyCharacteristic {
                guard peripheral.updateValue(
                    fragment,
                    for: notifyCharacteristic,
                    onSubscribedCentrals: [central]
                ) else {
                    pendingCentralWrites[uuid, default: []].append(fragment)
                    return
                }
                continue
            }
            return
        }
    }
}

extension MeshBluetoothTransport: @preconcurrency CBCentralManagerDelegate {
    public func centralManagerDidUpdateState(_ central: CBCentralManager) {
        refreshBluetoothState()
    }

    public func centralManager(
        _ central: CBCentralManager,
        didDiscover remote: CBPeripheral,
        advertisementData: [String: Any],
        rssi RSSI: NSNumber
    ) {
        guard discovered[remote.identifier] == nil else { return }
        discovered[remote.identifier] = remote
        remote.delegate = self
        central.connect(remote)
    }

    public func centralManager(_ central: CBCentralManager, didConnect remote: CBPeripheral) {
        remote.discoverServices([Self.serviceUUID])
    }

    public func centralManager(
        _ central: CBCentralManager,
        didDisconnectPeripheral remote: CBPeripheral,
        error: Error?
    ) {
        let peerID = remote.identifier.uuidString
        codec.peerDisconnected(peerID)
        discovered.removeValue(forKey: remote.identifier)
        writeCharacteristics.removeValue(forKey: remote.identifier)
        pendingPeripheralWrites.removeValue(forKey: remote.identifier)
        onPeerDisconnected?(peerID)
        if running {
            central.scanForPeripherals(withServices: [Self.serviceUUID])
        }
    }
}

extension MeshBluetoothTransport: @preconcurrency CBPeripheralDelegate {
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard error == nil else { return }
        for service in peripheral.services ?? [] where service.uuid == Self.serviceUUID {
            peripheral.discoverCharacteristics([Self.writeUUID, Self.notifyUUID], for: service)
        }
    }

    public func peripheral(
        _ peripheral: CBPeripheral,
        didDiscoverCharacteristicsFor service: CBService,
        error: Error?
    ) {
        guard error == nil else { return }
        for characteristic in service.characteristics ?? [] {
            if characteristic.uuid == Self.writeUUID {
                writeCharacteristics[peripheral.identifier] = characteristic
            } else if characteristic.uuid == Self.notifyUUID {
                peripheral.setNotifyValue(true, for: characteristic)
            }
        }
        guard writeCharacteristics[peripheral.identifier] != nil else { return }
        let mtu = peripheral.maximumWriteValueLength(for: .withoutResponse)
        register(peerID: peripheral.identifier.uuidString, mtu: mtu)
    }

    public func peripheral(
        _ peripheral: CBPeripheral,
        didUpdateValueFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        guard error == nil,
              characteristic.uuid == Self.notifyUUID,
              let value = characteristic.value else { return }
        receive(value, from: peripheral.identifier.uuidString)
    }

    public func peripheralIsReady(toSendWriteWithoutResponse peripheral: CBPeripheral) {
        let id = peripheral.identifier
        guard let characteristic = writeCharacteristics[id] else { return }
        while peripheral.canSendWriteWithoutResponse,
              var queue = pendingPeripheralWrites[id],
              !queue.isEmpty {
            let value = queue.removeFirst()
            pendingPeripheralWrites[id] = queue
            peripheral.writeValue(value, for: characteristic, type: .withoutResponse)
        }
        drainOutbound(to: id.uuidString)
    }
}

extension MeshBluetoothTransport: @preconcurrency CBPeripheralManagerDelegate {
    public func peripheralManagerDidUpdateState(_ peripheral: CBPeripheralManager) {
        refreshBluetoothState()
    }

    public func peripheralManager(
        _ peripheral: CBPeripheralManager,
        didAdd service: CBService,
        error: Error?
    ) {
        guard running, error == nil else { return }
        peripheral.startAdvertising([
            CBAdvertisementDataServiceUUIDsKey: [Self.serviceUUID],
            CBAdvertisementDataLocalNameKey: "nipworker"
        ])
    }

    public func peripheralManager(
        _ peripheral: CBPeripheralManager,
        central: CBCentral,
        didSubscribeTo characteristic: CBCharacteristic
    ) {
        let peerID = central.identifier.uuidString
        subscribedCentrals[central.identifier] = central
        register(peerID: peerID, mtu: central.maximumUpdateValueLength)
    }

    public func peripheralManager(
        _ peripheral: CBPeripheralManager,
        central: CBCentral,
        didUnsubscribeFrom characteristic: CBCharacteristic
    ) {
        let peerID = central.identifier.uuidString
        subscribedCentrals.removeValue(forKey: central.identifier)
        pendingCentralWrites.removeValue(forKey: central.identifier)
        codec.peerDisconnected(peerID)
        onPeerDisconnected?(peerID)
    }

    public func peripheralManager(
        _ peripheral: CBPeripheralManager,
        didReceiveWrite requests: [CBATTRequest]
    ) {
        for request in requests {
            guard request.characteristic.uuid == Self.writeUUID,
                  let value = request.value else {
                peripheral.respond(to: request, withResult: .requestNotSupported)
                continue
            }
            let peerID = request.central.identifier.uuidString
            if subscribedCentrals[request.central.identifier] == nil {
                subscribedCentrals[request.central.identifier] = request.central
                register(peerID: peerID, mtu: request.central.maximumUpdateValueLength)
            }
            receive(value, from: peerID)
            peripheral.respond(to: request, withResult: .success)
        }
    }

    public func peripheralManagerIsReady(toUpdateSubscribers peripheral: CBPeripheralManager) {
        guard let notifyCharacteristic else { return }
        for (id, central) in subscribedCentrals {
            while var queue = pendingCentralWrites[id], !queue.isEmpty {
                let value = queue[0]
                guard peripheral.updateValue(
                    value,
                    for: notifyCharacteristic,
                    onSubscribedCentrals: [central]
                ) else {
                    pendingCentralWrites[id] = queue
                    return
                }
                queue.removeFirst()
                pendingCentralWrites[id] = queue
            }
            drainOutbound(to: id.uuidString)
        }
    }
}
#endif
