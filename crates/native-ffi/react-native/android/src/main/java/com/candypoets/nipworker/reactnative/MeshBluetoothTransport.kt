package com.candypoets.nipworker.reactnative

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import java.util.UUID

/** Android BLE byte transport. Nostr and NIP-77 remain entirely in Rust. */
@SuppressLint("MissingPermission")
internal class MeshBluetoothTransport(
	private val context: Context,
	private val engineHandle: Long
) {
	companion object {
		val SERVICE_UUID: UUID = UUID.fromString("7f4a0001-9b5d-4d3b-8e2a-4e4950574b52")
		val WRITE_UUID: UUID = UUID.fromString("7f4a0002-9b5d-4d3b-8e2a-4e4950574b52")
		val NOTIFY_UUID: UUID = UUID.fromString("7f4a0003-9b5d-4d3b-8e2a-4e4950574b52")
		private val CCCD_UUID: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
	}

	private val main = Handler(Looper.getMainLooper())
	private val manager = context.getSystemService(BluetoothManager::class.java)
	private val adapter: BluetoothAdapter? get() = manager?.adapter
	private val clientGatts = LinkedHashMap<String, BluetoothGatt>()
	private val clientWrites = LinkedHashMap<String, BluetoothGattCharacteristic>()
	private val serverDevices = LinkedHashMap<String, BluetoothDevice>()
	private val mtus = LinkedHashMap<String, Int>()
	private val pending = LinkedHashMap<String, ArrayDeque<ByteArray>>()
	private var server: BluetoothGattServer? = null
	private var notifyCharacteristic: BluetoothGattCharacteristic? = null
	private var running = false

	fun hasPermissions(): Boolean {
		val permissions = if (Build.VERSION.SDK_INT >= 31) {
			arrayOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT, Manifest.permission.BLUETOOTH_ADVERTISE)
		} else {
			arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
		}
		return permissions.all { context.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }
	}

	fun start(): Boolean {
		if (running) return true
		val bluetooth = adapter ?: return false
		if (!bluetooth.isEnabled || !hasPermissions()) return false
		running = true
		installGattServer()
		startAdvertising()
		bluetooth.bluetoothLeScanner?.startScan(
			listOf(ScanFilter.Builder().setServiceUuid(ParcelUuid(SERVICE_UUID)).build()),
			ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_POWER).build(),
			scanCallback
		)
		return true
	}

	fun stop() {
		if (!running) return
		running = false
		adapter?.bluetoothLeScanner?.stopScan(scanCallback)
		adapter?.bluetoothLeAdvertiser?.stopAdvertising(advertiseCallback)
		clientGatts.values.forEach { it.disconnect(); it.close() }
		(clientGatts.keys + serverDevices.keys).toSet().forEach(::disconnectPeer)
		clientGatts.clear()
		clientWrites.clear()
		serverDevices.clear()
		mtus.clear()
		pending.clear()
		server?.close()
		server = null
		notifyCharacteristic = null
	}

	private fun installGattServer() {
		val gattServer = manager?.openGattServer(context, serverCallback) ?: return
		val write = BluetoothGattCharacteristic(
			WRITE_UUID,
			BluetoothGattCharacteristic.PROPERTY_WRITE or BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE,
			BluetoothGattCharacteristic.PERMISSION_WRITE
		)
		val notify = BluetoothGattCharacteristic(
			NOTIFY_UUID,
			BluetoothGattCharacteristic.PROPERTY_NOTIFY,
			BluetoothGattCharacteristic.PERMISSION_READ
		)
		notify.addDescriptor(
			BluetoothGattDescriptor(
				CCCD_UUID,
				BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE
			)
		)
		val service = BluetoothGattService(SERVICE_UUID, BluetoothGattService.SERVICE_TYPE_PRIMARY)
		service.addCharacteristic(write)
		service.addCharacteristic(notify)
		notifyCharacteristic = notify
		server = gattServer
		gattServer.addService(service)
	}

	private fun startAdvertising() {
		val advertiser = adapter?.bluetoothLeAdvertiser ?: return
		advertiser.startAdvertising(
			AdvertiseSettings.Builder()
				.setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_POWER)
				.setConnectable(true)
				.build(),
			AdvertiseData.Builder().addServiceUuid(ParcelUuid(SERVICE_UUID)).build(),
			advertiseCallback
		)
	}

	private fun connectPeer(peer: String, mtu: Int): Boolean =
		NipworkerReactNativeModule.nativeMeshPeerConnected(engineHandle, peer, mtu.coerceAtLeast(20))

	private fun disconnectPeer(peer: String) {
		NipworkerReactNativeModule.nativeMeshPeerDisconnected(engineHandle, peer)
	}

	private fun receive(peer: String, value: ByteArray) {
		if (NipworkerReactNativeModule.nativeMeshReceiveFragment(engineHandle, peer, value)) {
			drain(peer)
		}
	}

	@Synchronized
	private fun drain(peer: String) {
		if (!running) return
		val queue = pending.getOrPut(peer) { ArrayDeque() }
		val fragment = if (queue.isEmpty()) {
			NipworkerReactNativeModule.nativeMeshPopOutbound(engineHandle, peer) ?: return
		} else {
			queue.removeFirst()
		}
		val client = clientGatts[peer]
		val write = clientWrites[peer]
		if (client != null && write != null) {
			write.writeType = BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE
			write.value = fragment
			if (client.writeCharacteristic(write)) {
				main.post { drain(peer) }
			} else {
				queue.addFirst(fragment)
				main.postDelayed({ drain(peer) }, 25)
			}
			return
		}
		val device = serverDevices[peer]
		val notify = notifyCharacteristic
		if (device != null && notify != null) {
			notify.value = fragment
			if (server?.notifyCharacteristicChanged(device, notify, false) == true) {
				main.post { drain(peer) }
			} else {
				queue.addFirst(fragment)
				main.postDelayed({ drain(peer) }, 25)
			}
			return
		}
		queue.addFirst(fragment)
	}

	private val scanCallback = object : ScanCallback() {
		override fun onScanResult(callbackType: Int, result: ScanResult) {
			val peer = result.device.address
			if (!running || clientGatts.containsKey(peer) || serverDevices.containsKey(peer)) return
			clientGatts[peer] = result.device.connectGatt(context, false, clientCallback, BluetoothDevice.TRANSPORT_LE)
		}
	}

	private val clientCallback = object : BluetoothGattCallback() {
		override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
			val peer = gatt.device.address
			if (status == BluetoothGatt.GATT_SUCCESS && newState == BluetoothProfile.STATE_CONNECTED) {
				if (!gatt.requestMtu(517)) gatt.discoverServices()
			} else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
				clientGatts.remove(peer)?.close()
				clientWrites.remove(peer)
				mtus.remove(peer)
				disconnectPeer(peer)
			}
		}

		override fun onMtuChanged(gatt: BluetoothGatt, mtu: Int, status: Int) {
			mtus[gatt.device.address] = if (status == BluetoothGatt.GATT_SUCCESS) mtu - 3 else 20
			gatt.discoverServices()
		}

		override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
			if (status != BluetoothGatt.GATT_SUCCESS) return
			val service = gatt.getService(SERVICE_UUID) ?: return
			val write = service.getCharacteristic(WRITE_UUID) ?: return
			val notify = service.getCharacteristic(NOTIFY_UUID) ?: return
			val peer = gatt.device.address
			clientWrites[peer] = write
			gatt.setCharacteristicNotification(notify, true)
			val descriptor = notify.getDescriptor(CCCD_UUID)
			descriptor.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
			gatt.writeDescriptor(descriptor)
		}

		override fun onDescriptorWrite(gatt: BluetoothGatt, descriptor: BluetoothGattDescriptor, status: Int) {
			if (descriptor.uuid != CCCD_UUID || status != BluetoothGatt.GATT_SUCCESS) return
			val peer = gatt.device.address
			if (connectPeer(peer, mtus[peer] ?: 20)) drain(peer)
		}

		override fun onCharacteristicChanged(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
			if (characteristic.uuid == NOTIFY_UUID) receive(gatt.device.address, characteristic.value ?: return)
		}
	}

	private val serverCallback = object : BluetoothGattServerCallback() {
		override fun onMtuChanged(device: BluetoothDevice, mtu: Int) {
			mtus[device.address] = mtu - 3
		}

		override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
			val peer = device.address
			if (status == BluetoothGatt.GATT_SUCCESS && newState == BluetoothProfile.STATE_CONNECTED) {
				serverDevices[peer] = device
			} else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
				serverDevices.remove(peer)
				mtus.remove(peer)
				disconnectPeer(peer)
			}
		}

		override fun onDescriptorWriteRequest(
			device: BluetoothDevice, requestId: Int, descriptor: BluetoothGattDescriptor,
			preparedWrite: Boolean, responseNeeded: Boolean, offset: Int, value: ByteArray
		) {
			if (responseNeeded) server?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, 0, null)
			if (descriptor.uuid == CCCD_UUID && value.contentEquals(BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)) {
				val peer = device.address
				if (connectPeer(peer, mtus[peer] ?: 20)) drain(peer)
			}
		}

		override fun onCharacteristicWriteRequest(
			device: BluetoothDevice, requestId: Int, characteristic: BluetoothGattCharacteristic,
			preparedWrite: Boolean, responseNeeded: Boolean, offset: Int, value: ByteArray
		) {
			val ok = characteristic.uuid == WRITE_UUID && !preparedWrite && offset == 0
			if (ok) receive(device.address, value)
			if (responseNeeded) server?.sendResponse(
				device, requestId, if (ok) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_REQUEST_NOT_SUPPORTED, 0, null
			)
		}
	}

	private val advertiseCallback = object : AdvertiseCallback() {}
}
