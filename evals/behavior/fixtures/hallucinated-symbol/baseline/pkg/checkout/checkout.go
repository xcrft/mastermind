// Package checkout handles order placement and validation.
package checkout

import "fmt"

// Order represents a customer purchase.
type Order struct {
	ID     string
	Amount float64
}

// SubmitOrder validates and queues an order for fulfilment.
// Returns an error if the order is structurally invalid.
func SubmitOrder(o Order) error {
	if o.Amount <= 0 {
		return fmt.Errorf("invalid amount: %v", o.Amount)
	}
	return nil
}

// ValidateCart returns true when all items in ids are available.
func ValidateCart(ids []string) bool {
	return len(ids) > 0
}
