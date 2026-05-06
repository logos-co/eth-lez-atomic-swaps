#ifndef SWAP_UI_INTERFACE_H
#define SWAP_UI_INTERFACE_H

#include <QObject>
#include <QString>
#include "interface.h"

class SwapUiInterface : public PluginInterface
{
public:
    virtual ~SwapUiInterface() = default;
};

#define SwapUiInterface_iid "org.logos.SwapUiInterface"
Q_DECLARE_INTERFACE(SwapUiInterface, SwapUiInterface_iid)

#endif // SWAP_UI_INTERFACE_H
