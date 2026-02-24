#ifndef I_LEZ_ATOMIC_SWAP_MODULE_H
#define I_LEZ_ATOMIC_SWAP_MODULE_H

#include <QtPlugin>

class ILezAtomicSwapModule {
public:
    virtual ~ILezAtomicSwapModule() = default;
    virtual void initLogos() = 0;
};

#define ILezAtomicSwapModule_iid "org.logos.ilezatomicswapmodule"
Q_DECLARE_INTERFACE(ILezAtomicSwapModule, ILezAtomicSwapModule_iid)

#endif // I_LEZ_ATOMIC_SWAP_MODULE_H
