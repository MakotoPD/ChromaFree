#pragma once

template <class Interface = IMFAttributes>
struct AttributesBase : public Interface
{
protected:
    wil::com_ptr_nothrow<IMFAttributes> _attributes;

    AttributesBase()
    {
        THROW_IF_FAILED(MFCreateAttributes(&_attributes, 0));
    }

public:
    STDMETHODIMP GetItem(REFGUID key, PROPVARIANT* value) override { return _attributes->GetItem(key, value); }
    STDMETHODIMP GetItemType(REFGUID key, MF_ATTRIBUTE_TYPE* type) override { return _attributes->GetItemType(key, type); }
    STDMETHODIMP CompareItem(REFGUID key, REFPROPVARIANT value, BOOL* result) override { return _attributes->CompareItem(key, value, result); }
    STDMETHODIMP Compare(IMFAttributes* theirs, MF_ATTRIBUTES_MATCH_TYPE type, BOOL* result) override { return _attributes->Compare(theirs, type, result); }
    STDMETHODIMP GetUINT32(REFGUID key, UINT32* value) override { return _attributes->GetUINT32(key, value); }
    STDMETHODIMP GetUINT64(REFGUID key, UINT64* value) override { return _attributes->GetUINT64(key, value); }
    STDMETHODIMP GetDouble(REFGUID key, double* value) override { return _attributes->GetDouble(key, value); }
    STDMETHODIMP GetGUID(REFGUID key, GUID* value) override { return _attributes->GetGUID(key, value); }
    STDMETHODIMP GetStringLength(REFGUID key, UINT32* length) override { return _attributes->GetStringLength(key, length); }
    STDMETHODIMP GetString(REFGUID key, LPWSTR value, UINT32 size, UINT32* length) override { return _attributes->GetString(key, value, size, length); }
    STDMETHODIMP GetAllocatedString(REFGUID key, LPWSTR* value, UINT32* length) override { return _attributes->GetAllocatedString(key, value, length); }
    STDMETHODIMP GetBlobSize(REFGUID key, UINT32* size) override { return _attributes->GetBlobSize(key, size); }
    STDMETHODIMP GetBlob(REFGUID key, UINT8* buffer, UINT32 size, UINT32* written) override { return _attributes->GetBlob(key, buffer, size, written); }
    STDMETHODIMP GetAllocatedBlob(REFGUID key, UINT8** buffer, UINT32* size) override { return _attributes->GetAllocatedBlob(key, buffer, size); }
    STDMETHODIMP GetUnknown(REFGUID key, REFIID iid, LPVOID* object) override { return _attributes->GetUnknown(key, iid, object); }
    STDMETHODIMP SetItem(REFGUID key, REFPROPVARIANT value) override { return _attributes->SetItem(key, value); }
    STDMETHODIMP DeleteItem(REFGUID key) override { return _attributes->DeleteItem(key); }
    STDMETHODIMP DeleteAllItems() override { return _attributes->DeleteAllItems(); }
    STDMETHODIMP SetUINT32(REFGUID key, UINT32 value) override { return _attributes->SetUINT32(key, value); }
    STDMETHODIMP SetUINT64(REFGUID key, UINT64 value) override { return _attributes->SetUINT64(key, value); }
    STDMETHODIMP SetDouble(REFGUID key, double value) override { return _attributes->SetDouble(key, value); }
    STDMETHODIMP SetGUID(REFGUID key, REFGUID value) override { return _attributes->SetGUID(key, value); }
    STDMETHODIMP SetString(REFGUID key, LPCWSTR value) override { return _attributes->SetString(key, value); }
    STDMETHODIMP SetBlob(REFGUID key, const UINT8* buffer, UINT32 size) override { return _attributes->SetBlob(key, buffer, size); }
    STDMETHODIMP SetUnknown(REFGUID key, IUnknown* value) override { return _attributes->SetUnknown(key, value); }
    STDMETHODIMP LockStore() override { return _attributes->LockStore(); }
    STDMETHODIMP UnlockStore() override { return _attributes->UnlockStore(); }
    STDMETHODIMP GetCount(UINT32* count) override { return _attributes->GetCount(count); }
    STDMETHODIMP GetItemByIndex(UINT32 index, GUID* key, PROPVARIANT* value) override { return _attributes->GetItemByIndex(index, key, value); }
    STDMETHODIMP CopyAllItems(IMFAttributes* destination) override { return _attributes->CopyAllItems(destination); }
};
